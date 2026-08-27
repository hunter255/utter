//! Microphone authorization without opening an audio stream.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrophonePermission {
    NotDetermined,
    Granted,
    Denied,
    Unavailable,
}

#[cfg(target_os = "macos")]
mod platform {
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{
        AVAuthorizationStatus, AVCaptureDevice, AVMediaType, AVMediaTypeAudio,
    };

    use super::MicrophonePermission;

    const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

    fn audio_media_type() -> Option<&'static AVMediaType> {
        // AVMediaTypeAudio is an AVFoundation framework constant and is
        // present on every supported macOS version.
        unsafe { AVMediaTypeAudio }
    }

    pub(super) fn map_status(status: AVAuthorizationStatus) -> MicrophonePermission {
        match status {
            AVAuthorizationStatus::NotDetermined => MicrophonePermission::NotDetermined,
            AVAuthorizationStatus::Authorized => MicrophonePermission::Granted,
            AVAuthorizationStatus::Denied | AVAuthorizationStatus::Restricted => {
                MicrophonePermission::Denied
            }
            _ => MicrophonePermission::Unavailable,
        }
    }

    pub(super) fn status() -> MicrophonePermission {
        let Some(media_type) = audio_media_type() else {
            return MicrophonePermission::Unavailable;
        };
        map_status(unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) })
    }

    pub(super) fn request() -> MicrophonePermission {
        match status() {
            MicrophonePermission::NotDetermined => {}
            current => return current,
        }

        let Some(media_type) = audio_media_type() else {
            return MicrophonePermission::Unavailable;
        };
        let (tx, rx) = mpsc::sync_channel(1);
        let handler: RcBlock<dyn Fn(Bool)> = RcBlock::new(move |granted: Bool| {
            let _ = tx.send(granted.as_bool());
        });
        unsafe {
            AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &handler)
        };

        match rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(true) => MicrophonePermission::Granted,
            Ok(false) => MicrophonePermission::Denied,
            Err(_) => MicrophonePermission::Unavailable,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn maps_every_apple_authorization_status() {
            assert_eq!(
                map_status(AVAuthorizationStatus::NotDetermined),
                MicrophonePermission::NotDetermined
            );
            assert_eq!(
                map_status(AVAuthorizationStatus::Authorized),
                MicrophonePermission::Granted
            );
            assert_eq!(
                map_status(AVAuthorizationStatus::Denied),
                MicrophonePermission::Denied
            );
            assert_eq!(
                map_status(AVAuthorizationStatus::Restricted),
                MicrophonePermission::Denied
            );
            assert_eq!(
                map_status(AVAuthorizationStatus(99)),
                MicrophonePermission::Unavailable
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::MicrophonePermission;

    pub(super) fn status() -> MicrophonePermission {
        MicrophonePermission::Unavailable
    }

    pub(super) fn request() -> MicrophonePermission {
        MicrophonePermission::Unavailable
    }
}

pub fn microphone_permission() -> MicrophonePermission {
    platform::status()
}

/// Requests access only when the OS still reports `NotDetermined`.
pub fn request_microphone_permission() -> MicrophonePermission {
    platform::request()
}
