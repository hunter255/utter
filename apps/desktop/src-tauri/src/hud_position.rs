//! Places the HUD near the macOS insertion point without ever taking focus.
//!
//! Accessibility geometry is best-effort: native editors usually expose a
//! caret rectangle, while Electron/contenteditable controls sometimes expose
//! only the focused element. Missing permission, unsupported attributes and
//! timeouts all fall through to the pointer, then to the active screen.

#[derive(Debug, Clone, Copy, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Size {
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn right(self) -> f64 {
        self.x + self.width
    }

    fn bottom(self) -> f64 {
        self.y + self.height
    }

    fn center(self) -> Point {
        Point {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }

    fn is_usable(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 0.0
            && self.height > 0.0
    }
}

const SCREEN_PADDING: f64 = 8.0;
const CARET_GAP: f64 = 12.0;
const POINTER_OFFSET_X: f64 = 16.0;
const POINTER_OFFSET_Y: f64 = 20.0;
#[cfg(target_os = "macos")]
const DEFAULT_HUD_SIZE: Size = Size {
    width: 280.0,
    height: 104.0,
};

fn clamped_origin(mut point: Point, hud: Size, work: Rect) -> Point {
    let min_x = work.x + SCREEN_PADDING;
    let min_y = work.y + SCREEN_PADDING;
    let max_x = (work.right() - SCREEN_PADDING - hud.width).max(min_x);
    let max_y = (work.bottom() - SCREEN_PADDING - hud.height).max(min_y);
    point.x = point.x.clamp(min_x, max_x);
    point.y = point.y.clamp(min_y, max_y);
    point
}

fn near_rect(anchor: Rect, hud: Size, work: Rect) -> Point {
    let mut y = anchor.bottom() + CARET_GAP;
    if y + hud.height > work.bottom() - SCREEN_PADDING {
        y = anchor.y - CARET_GAP - hud.height;
    }
    clamped_origin(
        Point {
            x: anchor.center().x - hud.width / 2.0,
            y,
        },
        hud,
        work,
    )
}

fn near_pointer(pointer: Point, hud: Size, work: Rect) -> Point {
    let mut y = pointer.y + POINTER_OFFSET_Y;
    if y + hud.height > work.bottom() - SCREEN_PADDING {
        y = pointer.y - POINTER_OFFSET_Y - hud.height;
    }
    clamped_origin(
        Point {
            x: pointer.x + POINTER_OFFSET_X,
            y,
        },
        hud,
        work,
    )
}

fn bottom_center(hud: Size, work: Rect) -> Point {
    clamped_origin(
        Point {
            x: work.center().x - hud.width / 2.0,
            y: work.bottom() - SCREEN_PADDING - hud.height,
        },
        hud,
        work,
    )
}

fn focused_element_is_useful(element: Rect, work: Rect) -> bool {
    element.is_usable()
        && element.width <= work.width * 0.92
        && element.height <= work.height * 0.50
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::ptr;

    use accessibility_sys::{
        kAXBoundsForRangeParameterizedAttribute, kAXErrorCannotComplete, kAXErrorSuccess,
        kAXFocusedUIElementAttribute, kAXPositionAttribute, kAXSelectedTextRangeAttribute,
        kAXSizeAttribute, kAXValueTypeCFRange, kAXValueTypeCGPoint, kAXValueTypeCGRect,
        kAXValueTypeCGSize, AXIsProcessTrusted, AXUIElementCopyAttributeValue,
        AXUIElementCopyParameterizedAttributeValue, AXUIElementCreateSystemWide,
        AXUIElementGetTypeID, AXUIElementRef, AXUIElementSetMessagingTimeout, AXValueCreate,
        AXValueGetType, AXValueGetTypeID, AXValueGetValue,
    };
    use core_foundation::base::{CFRange, CFType, CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    use core_graphics_types::geometry::{CGPoint, CGRect, CGSize};
    use tauri::{AppHandle, LogicalPosition, Monitor, WebviewWindow};
    use utter_store::HudPlacement;

    use super::{
        bottom_center, focused_element_is_useful, near_pointer, near_rect, Point, Rect, Size,
        DEFAULT_HUD_SIZE,
    };

    #[derive(Default)]
    struct AnchorCandidates {
        caret: Option<Rect>,
        focused_element: Option<Rect>,
    }

    struct MessagingTimeout(AXUIElementRef);

    #[derive(Default)]
    struct AxQuery {
        timed_out: bool,
    }

    impl Drop for MessagingTimeout {
        fn drop(&mut self) {
            // SAFETY: the referenced system-wide element is still retained by
            // `accessibility_anchors`; this guard is dropped before it.
            unsafe {
                let _ = AXUIElementSetMessagingTimeout(self.0, 0.0);
            }
        }
    }

    fn owned_cf(reference: CFTypeRef) -> Option<CFType> {
        if reference.is_null() {
            None
        } else {
            // SAFETY: every caller passes a value returned by an AX Create or
            // Copy function, so this owns exactly the returned retain count.
            Some(unsafe { CFType::wrap_under_create_rule(reference) })
        }
    }

    fn copy_attribute(query: &mut AxQuery, element: AXUIElementRef, name: &str) -> Option<CFType> {
        if query.timed_out {
            return None;
        }
        let name = CFString::new(name);
        let mut value: CFTypeRef = ptr::null();
        // SAFETY: `element` is a live retained AXUIElement and `value` is a
        // valid out pointer. Success follows Core Foundation's Copy rule.
        let error = unsafe {
            AXUIElementCopyAttributeValue(element, name.as_concrete_TypeRef(), &mut value)
        };
        query.timed_out = error == kAXErrorCannotComplete;
        let value = owned_cf(value)?;
        (error == kAXErrorSuccess).then_some(value)
    }

    fn copy_parameterized(
        query: &mut AxQuery,
        element: AXUIElementRef,
        name: &str,
        parameter: &CFType,
    ) -> Option<CFType> {
        if query.timed_out {
            return None;
        }
        let name = CFString::new(name);
        let mut value: CFTypeRef = ptr::null();
        // SAFETY: all references remain retained for the duration of the
        // synchronous AX call; `value` follows the Copy rule on success.
        let error = unsafe {
            AXUIElementCopyParameterizedAttributeValue(
                element,
                name.as_concrete_TypeRef(),
                parameter.as_CFTypeRef(),
                &mut value,
            )
        };
        query.timed_out = error == kAXErrorCannotComplete;
        let value = owned_cf(value)?;
        (error == kAXErrorSuccess).then_some(value)
    }

    fn ax_element(value: &CFType) -> Option<AXUIElementRef> {
        // SAFETY: asking for a Core Foundation type id has no side effects.
        (value.type_of() == unsafe { AXUIElementGetTypeID() })
            .then(|| value.as_CFTypeRef().cast_mut().cast())
    }

    fn ax_value(value: &CFType, expected_type: u32, output: *mut c_void) -> bool {
        // SAFETY: type IDs are queried before the cast, and each caller
        // supplies storage matching `expected_type`'s documented payload.
        unsafe {
            if value.type_of() != AXValueGetTypeID() {
                return false;
            }
            let reference = value
                .as_CFTypeRef()
                .cast_mut()
                .cast::<accessibility_sys::__AXValue>();
            AXValueGetType(reference) == expected_type
                && AXValueGetValue(reference, expected_type, output)
        }
    }

    fn range_from(value: &CFType) -> Option<CFRange> {
        let mut range = CFRange {
            location: 0,
            length: 0,
        };
        ax_value(
            value,
            kAXValueTypeCFRange,
            (&mut range as *mut CFRange).cast(),
        )
        .then_some(range)
    }

    fn rect_from(value: &CFType) -> Option<Rect> {
        let mut rect = CGRect::default();
        if !ax_value(value, kAXValueTypeCGRect, (&mut rect as *mut CGRect).cast()) {
            return None;
        }
        let rect = Rect {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        };
        rect.is_usable().then_some(rect)
    }

    fn point_from(value: &CFType) -> Option<CGPoint> {
        let mut point = CGPoint::default();
        ax_value(
            value,
            kAXValueTypeCGPoint,
            (&mut point as *mut CGPoint).cast(),
        )
        .then_some(point)
    }

    fn size_from(value: &CFType) -> Option<CGSize> {
        let mut size = CGSize::default();
        ax_value(value, kAXValueTypeCGSize, (&mut size as *mut CGSize).cast()).then_some(size)
    }

    fn range_value(range: CFRange) -> Option<CFType> {
        // SAFETY: the pointer refers to a correctly laid out CFRange for the
        // duration of the call; AXValueCreate retains a copy of its bytes.
        let value = unsafe {
            AXValueCreate(
                kAXValueTypeCFRange,
                (&range as *const CFRange).cast::<c_void>(),
            )
        };
        owned_cf(value.cast())
    }

    fn bounds_for_range(
        query: &mut AxQuery,
        element: AXUIElementRef,
        range: CFRange,
    ) -> Option<Rect> {
        let parameter = range_value(range)?;
        let value = copy_parameterized(
            query,
            element,
            kAXBoundsForRangeParameterizedAttribute,
            &parameter,
        )?;
        rect_from(&value)
    }

    fn caret_rect(query: &mut AxQuery, element: AXUIElementRef) -> Option<Rect> {
        let selection = copy_attribute(query, element, kAXSelectedTextRangeAttribute)?;
        let selection = range_from(&selection)?;
        if selection.location < 0 || selection.length < 0 {
            return None;
        }
        let caret = selection.location.checked_add(selection.length)?;

        if let Some(bounds) = bounds_for_range(
            query,
            element,
            CFRange {
                location: caret,
                length: 0,
            },
        ) {
            return Some(bounds);
        }

        // Some controls reject zero-length ranges. A character to the right
        // gives the caret's left edge; at end-of-text, the character to the
        // left gives its right edge. Neither path reads the field's text.
        if let Some(right) = bounds_for_range(
            query,
            element,
            CFRange {
                location: caret,
                length: 1,
            },
        ) {
            return Some(Rect {
                width: 0.0,
                ..right
            });
        }
        if caret > 0 {
            if let Some(left) = bounds_for_range(
                query,
                element,
                CFRange {
                    location: caret - 1,
                    length: 1,
                },
            ) {
                return Some(Rect {
                    x: left.right(),
                    width: 0.0,
                    ..left
                });
            }
        }
        None
    }

    fn element_rect(query: &mut AxQuery, element: AXUIElementRef) -> Option<Rect> {
        let point = point_from(&copy_attribute(query, element, kAXPositionAttribute)?)?;
        let size = size_from(&copy_attribute(query, element, kAXSizeAttribute)?)?;
        let rect = Rect {
            x: point.x,
            y: point.y,
            width: size.width,
            height: size.height,
        };
        rect.is_usable().then_some(rect)
    }

    fn accessibility_anchors() -> AnchorCandidates {
        // SAFETY: this is the non-prompting system trust probe.
        if !unsafe { AXIsProcessTrusted() } {
            return AnchorCandidates::default();
        }

        // SAFETY: Create returns a retained system-wide AX element or null.
        let system = unsafe { AXUIElementCreateSystemWide() };
        let Some(system_owner) = owned_cf(system.cast()) else {
            return AnchorCandidates::default();
        };
        let Some(system) = ax_element(&system_owner) else {
            return AnchorCandidates::default();
        };

        // Apple documents that a timeout set on the system-wide element is
        // global to this process. That also covers the focused child element
        // returned below; setting it on a separate application object would
        // not cover other AX objects, even when they compare equal.
        // SAFETY: `system` is retained by `system_owner` for this scope.
        if unsafe { AXUIElementSetMessagingTimeout(system, 0.15) } != kAXErrorSuccess {
            return AnchorCandidates::default();
        }
        let _timeout = MessagingTimeout(system);
        let mut query = AxQuery::default();

        let Some(focused_owner) = copy_attribute(&mut query, system, kAXFocusedUIElementAttribute)
        else {
            return AnchorCandidates::default();
        };
        let Some(focused) = ax_element(&focused_owner) else {
            return AnchorCandidates::default();
        };

        AnchorCandidates {
            caret: caret_rect(&mut query, focused),
            focused_element: element_rect(&mut query, focused),
        }
    }

    fn work_area(monitor: &Monitor) -> Rect {
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        let position = area.position.to_logical::<f64>(scale);
        let size = area.size.to_logical::<f64>(scale);
        Rect {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        }
    }

    fn hud_size(hud: &WebviewWindow) -> Size {
        let Ok(scale) = hud.scale_factor() else {
            return DEFAULT_HUD_SIZE;
        };
        let Ok(size) = hud.outer_size() else {
            return DEFAULT_HUD_SIZE;
        };
        let size = size.to_logical::<f64>(scale);
        Size {
            width: size.width,
            height: size.height,
        }
    }

    fn monitor_for(hud: &WebviewWindow, point: Point) -> Option<Monitor> {
        hud.monitor_from_point(point.x, point.y).ok().flatten()
    }

    fn pointer(app: &AppHandle) -> Option<Point> {
        let physical = app.cursor_position().ok()?;
        let primary_scale = app
            .primary_monitor()
            .ok()
            .flatten()
            .map_or(1.0, |monitor| monitor.scale_factor());
        let logical = physical.to_logical::<f64>(primary_scale);
        Some(Point {
            x: logical.x,
            y: logical.y,
        })
    }

    fn fallback_monitor(
        app: &AppHandle,
        hud: &WebviewWindow,
        pointer: Option<Point>,
    ) -> Option<Monitor> {
        pointer
            .and_then(|point| monitor_for(hud, point))
            .or_else(|| hud.current_monitor().ok().flatten())
            .or_else(|| app.primary_monitor().ok().flatten())
    }

    fn calculate_position(
        app: &AppHandle,
        hud: &WebviewWindow,
        placement: HudPlacement,
    ) -> Option<Point> {
        let hud_size = hud_size(hud);

        let follow_pointer = match placement {
            HudPlacement::Auto => {
                let anchors = accessibility_anchors();

                if let Some(caret) = anchors.caret {
                    if let Some(monitor) = monitor_for(hud, caret.center()) {
                        return Some(near_rect(caret, hud_size, work_area(&monitor)));
                    }
                }

                if let Some(element) = anchors.focused_element {
                    if let Some(monitor) = monitor_for(hud, element.center()) {
                        let work = work_area(&monitor);
                        if focused_element_is_useful(element, work) {
                            return Some(near_rect(element, hud_size, work));
                        }
                    }
                }

                true
            }
            HudPlacement::Pointer => true,
            HudPlacement::BottomCenter => false,
        };

        let pointer = pointer(app);
        if follow_pointer {
            if let Some(pointer) = pointer {
                if let Some(monitor) = monitor_for(hud, pointer) {
                    return Some(near_pointer(pointer, hud_size, work_area(&monitor)));
                }
            }
        }

        let monitor = fallback_monitor(app, hud, pointer)?;
        Some(bottom_center(hud_size, work_area(&monitor)))
    }

    pub(super) fn position_hud(
        app: &AppHandle,
        hud: &WebviewWindow,
        placement: HudPlacement,
    ) -> Result<(), String> {
        let point = calculate_position(app, hud, placement)
            .ok_or_else(|| "no monitor is available for HUD positioning".to_string())?;
        hud.set_position(LogicalPosition::new(point.x, point.y))
            .map_err(|error| error.to_string())
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn position_hud(
    app: &tauri::AppHandle,
    hud: &tauri::WebviewWindow,
    placement: utter_store::HudPlacement,
) -> Result<(), String> {
    macos::position_hud(app, hud, placement)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUD: Size = Size {
        width: 280.0,
        height: 104.0,
    };
    const WORK: Rect = Rect {
        x: 0.0,
        y: 24.0,
        width: 1512.0,
        height: 930.0,
    };

    #[test]
    fn caret_places_hud_below_and_centered() {
        let point = near_rect(
            Rect {
                x: 700.0,
                y: 300.0,
                width: 2.0,
                height: 18.0,
            },
            HUD,
            WORK,
        );
        assert_eq!(point, Point { x: 561.0, y: 330.0 });
    }

    #[test]
    fn caret_near_bottom_flips_hud_above() {
        let point = near_rect(
            Rect {
                x: 700.0,
                y: 900.0,
                width: 2.0,
                height: 18.0,
            },
            HUD,
            WORK,
        );
        assert_eq!(point.y, 784.0);
    }

    #[test]
    fn placement_clamps_to_each_work_area_edge() {
        let top_left = near_rect(
            Rect {
                x: -100.0,
                y: -100.0,
                width: 0.0,
                height: 10.0,
            },
            HUD,
            WORK,
        );
        assert_eq!(top_left, Point { x: 8.0, y: 32.0 });

        let bottom_right = near_pointer(
            Point {
                x: 2000.0,
                y: 1200.0,
            },
            HUD,
            WORK,
        );
        assert_eq!(
            bottom_right,
            Point {
                x: 1224.0,
                y: 842.0
            }
        );
    }

    #[test]
    fn negative_monitor_coordinates_are_preserved() {
        let work = Rect {
            x: -1920.0,
            y: -200.0,
            width: 1920.0,
            height: 1080.0,
        };
        let point = near_pointer(
            Point {
                x: -1800.0,
                y: -100.0,
            },
            HUD,
            work,
        );
        assert_eq!(
            point,
            Point {
                x: -1784.0,
                y: -80.0
            }
        );
    }

    #[test]
    fn bottom_center_uses_the_visible_work_area() {
        assert_eq!(bottom_center(HUD, WORK), Point { x: 616.0, y: 842.0 });
    }

    #[test]
    fn almost_full_screen_ax_elements_are_rejected() {
        assert!(!focused_element_is_useful(
            Rect {
                x: 0.0,
                y: 24.0,
                width: 1500.0,
                height: 920.0,
            },
            WORK,
        ));
        assert!(focused_element_is_useful(
            Rect {
                x: 300.0,
                y: 200.0,
                width: 700.0,
                height: 400.0,
            },
            WORK,
        ));
        assert!(!focused_element_is_useful(
            Rect {
                x: 100.0,
                y: 80.0,
                width: 1200.0,
                height: 800.0,
            },
            WORK,
        ));
    }

    #[test]
    fn invalid_geometry_never_becomes_an_anchor() {
        assert!(!Rect {
            x: f64::NAN,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }
        .is_usable());
        assert!(!Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 0.0,
        }
        .is_usable());
    }
}
