//! macOS pasteboard transaction primitives.
//!
//! The transcript is published through an `NSPasteboardItemDataProvider`.
//! AppKit calls the provider when a paste target actually requests the text,
//! giving the injector a read receipt instead of making it guess with one
//! fixed sleep. `changeCount` separately protects clipboard ownership: if the
//! user copies something else while insertion is in flight, Utter must never
//! overwrite that newer value with the clipboard it saved earlier.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardItemDataProvider, NSPasteboardType,
    NSPasteboardTypeString, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString};

use crate::clipboard_receipt::{ReceiptDecision, ReceiptPolicy};

const RECEIPT_POLL_INTERVAL: Duration = Duration::from_millis(5);
const RECEIPT_QUIET_PERIOD: Duration = Duration::from_millis(25);
const RECEIPT_TIMEOUT: Duration = Duration::from_millis(750);
const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";

struct ProviderIvars {
    text: String,
    reads: Arc<AtomicUsize>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. All ivars are owned,
    // thread-safe Rust values and the class does not implement Drop.
    #[unsafe(super(NSObject))]
    #[ivars = ProviderIvars]
    struct PasteboardTextProvider;

    unsafe impl NSObjectProtocol for PasteboardTextProvider {}

    unsafe impl NSPasteboardItemDataProvider for PasteboardTextProvider {
        #[unsafe(method(pasteboard:item:provideDataForType:))]
        fn pasteboard_item_provide_data_for_type(
            &self,
            _pasteboard: Option<&NSPasteboard>,
            item: &NSPasteboardItem,
            requested_type: &NSPasteboardType,
        ) {
            let text = NSString::from_str(&self.ivars().text);
            if item.setString_forType(&text, requested_type) {
                self.ivars().reads.fetch_add(1, Ordering::Release);
            }
        }
    }
);

impl PasteboardTextProvider {
    fn new(text: &str, reads: Arc<AtomicUsize>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ProviderIvars {
            text: text.to_string(),
            reads,
        });
        // SAFETY: `this` is a freshly allocated NSObject subclass with all
        // Rust ivars initialized above.
        unsafe { msg_send![super(this), init] }
    }
}

/// Receipt and ownership token for one transient transcript publication.
pub(crate) struct PastePublication {
    reads: Option<Arc<AtomicUsize>>,
    baseline: usize,
    expected_change_count: isize,
    // Promised data can be requested later. Retaining both objects here makes
    // their lifetime explicit and lets a timeout materialize the string before
    // this transaction releases the provider.
    _provider: Option<Retained<PasteboardTextProvider>>,
    item: Option<Retained<NSPasteboardItem>>,
}

impl PastePublication {
    /// Publishes promised plain text and marks it as concealed for clipboard
    /// managers that implement the community convention used by arboard.
    pub(crate) fn publish(text: &str) -> Result<Self, String> {
        let pasteboard = NSPasteboard::generalPasteboard();
        let reads = Arc::new(AtomicUsize::new(0));
        let provider = PasteboardTextProvider::new(text, Arc::clone(&reads));
        let item = NSPasteboardItem::new();
        let string_types = NSArray::from_slice(&[unsafe { NSPasteboardTypeString }]);
        let provider_ref = ProtocolObject::<dyn NSPasteboardItemDataProvider>::from_ref(&*provider);

        if !item.setDataProvider_forTypes(provider_ref, &string_types) {
            return Err("NSPasteboardItem rejected its text data provider".to_string());
        }

        let concealed_type = NSString::from_str(CONCEALED_TYPE);
        let empty = NSString::from_str("");
        if !item.setString_forType(&empty, &concealed_type) {
            return Err("NSPasteboardItem rejected its concealed marker".to_string());
        }

        pasteboard.clearContents();
        let retained_item = item.clone();
        let writing_item = ProtocolObject::<dyn NSPasteboardWriting>::from_retained(item);
        let objects = NSArray::from_retained_slice(&[writing_item]);
        if !pasteboard.writeObjects(&objects) {
            return Err("NSPasteboard failed to publish the transcript".to_string());
        }

        Ok(Self {
            reads: Some(reads),
            baseline: 0,
            expected_change_count: pasteboard.changeCount(),
            _provider: Some(provider),
            item: Some(retained_item),
        })
    }

    /// Captures an ownership-only token after the normal arboard fallback.
    pub(crate) fn fallback() -> Self {
        Self {
            reads: None,
            baseline: 0,
            expected_change_count: NSPasteboard::generalPasteboard().changeCount(),
            _provider: None,
            item: None,
        }
    }

    /// Excludes any eager reads performed before the paste chord is posted.
    pub(crate) fn arm(&mut self) {
        self.baseline = self.read_count();
    }

    pub(crate) fn has_receipt_provider(&self) -> bool {
        self.reads.is_some()
    }

    pub(crate) fn still_owns_clipboard(&self) -> bool {
        NSPasteboard::generalPasteboard().changeCount() == self.expected_change_count
    }

    /// Waits for at least one post-arm read, followed by a short quiet period
    /// for targets that request the same representation more than once.
    pub(crate) fn wait_for_read(&self) -> ReadWaitOutcome {
        let started = Instant::now();
        let mut policy = ReceiptPolicy::new(self.baseline, RECEIPT_QUIET_PERIOD, RECEIPT_TIMEOUT);

        loop {
            match policy.observe(
                started.elapsed(),
                self.read_count(),
                self.still_owns_clipboard(),
            ) {
                ReceiptDecision::Pending => std::thread::sleep(RECEIPT_POLL_INTERVAL),
                ReceiptDecision::ReadConfirmed => return ReadWaitOutcome::Read,
                ReceiptDecision::OwnershipLost => return ReadWaitOutcome::OwnershipLost,
                ReceiptDecision::TimedOut => return ReadWaitOutcome::TimedOut,
            }
        }
    }

    /// Replaces the promise with concrete text before a no-receipt timeout
    /// releases the provider. This leaves the transcript available to a slow
    /// target instead of racing it with restored clipboard contents.
    pub(crate) fn materialize_after_timeout(&self) {
        let (Some(provider), Some(item)) = (&self._provider, &self.item) else {
            return;
        };
        let text = NSString::from_str(&provider.ivars().text);
        if !item.setString_forType(&text, unsafe { NSPasteboardTypeString }) {
            tracing::warn!(
                "utter-inject: failed to materialize transcript after paste receipt timeout"
            );
        }
    }

    fn read_count(&self) -> usize {
        self.reads
            .as_ref()
            .map_or(0, |reads| reads.load(Ordering::Acquire))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadWaitOutcome {
    Read,
    OwnershipLost,
    TimedOut,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Touches the real system clipboard, so it is intentionally manual.
    /// Verifies both the provider receipt and the `changeCount` ownership
    /// guard without synthesizing a paste key.
    #[test]
    #[ignore]
    fn promised_text_reports_reads_and_detects_replacement() {
        let _guard = crate::clipboard::SELECTION_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut selections = crate::clipboard::Selections::new();
        let original = selections.save();

        let result = (|| -> Result<(), String> {
            let mut publication = PastePublication::publish("Utter receipt test")?;
            publication.arm();

            let mut reader = arboard::Clipboard::new().map_err(|err| err.to_string())?;
            let text = reader.get_text().map_err(|err| err.to_string())?;
            if text != "Utter receipt test" {
                return Err(format!("promised text changed: {text:?}"));
            }
            if publication.wait_for_read() != ReadWaitOutcome::Read {
                return Err("promised read did not produce a receipt".to_string());
            }
            if !publication.still_owns_clipboard() {
                return Err("reading changed pasteboard ownership".to_string());
            }

            reader
                .set_text("newer user clipboard")
                .map_err(|err| err.to_string())?;
            if publication.still_owns_clipboard() {
                return Err("replacement did not change pasteboard ownership".to_string());
            }

            let timed_out = PastePublication::publish("Utter materialized timeout test")?;
            timed_out.materialize_after_timeout();
            drop(timed_out);
            let text = reader.get_text().map_err(|err| err.to_string())?;
            if text != "Utter materialized timeout test" {
                return Err(format!(
                    "materialized text changed after provider drop: {text:?}"
                ));
            }
            Ok(())
        })();

        selections.restore(original);
        result.expect("promised pasteboard transaction should work");
    }
}
