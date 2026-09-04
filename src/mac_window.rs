//! The main window's zoom. AppKit's animated zooms (`zoom:`,
//! `setFrame:display:animate:`) animate on the window server, from a
//! snapshot of the window: the interface never runs during them, so its
//! contents stretch across the screen on the way. The frame is stepped
//! here instead: every step is a plain resize, like a hand-dragged window
//! edge, which the interface redraws.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, sel};
use objc2_app_kit::{NSApplication, NSWindow, NSWorkspace};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSTimer,
};

/// How often the frame steps toward its target; every step is a resize the
/// interface redraws, so this is also how often it redraws. A 120th of a
/// second, the faster of the two display refresh rates.
const STEP: Duration = Duration::from_nanos(8_333_333);

/// The zoom in progress. Only the main thread touches it: the interface
/// and the timer's ticks both run there.
#[derive(Clone)]
struct Zoom {
    window: Retained<NSWindow>,
    start: NSRect,
    target: NSRect,
    started: Instant,
    /// How long the window takes to reach the target: what AppKit would
    /// use to animate the same change.
    duration: f64,
    timer: Retained<NSTimer>,
}

thread_local! {
    static ZOOM: RefCell<Option<Zoom>> = const { RefCell::new(None) };
    // The frame the window had before it was zoomed, restored on the next
    // zoom. Only the interface thread calls, and one window zooms at a
    // time.
    static RESTORE_FRAME: Cell<Option<NSRect>> = const { Cell::new(None) };
}

// Receives the timer's ticks. NSTimer keeps its target alive, so nothing
// else needs to hold one.
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "FastpotifyZoomTick"]
    struct ZoomTick;

    impl ZoomTick {
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            step();
        }
    }
);

/// What a zoom toggle should do with the window.
enum ZoomOutcome {
    /// Step the window to this frame.
    Step(NSRect),
    /// Hand the zoom to AppKit, which remembers its own frame to restore.
    Native,
}

/// Decides where a zoom toggle sends the window, and what stays remembered
/// as the pre-zoom frame: `running` is the zoom in flight, as its start
/// and target; `frame` is where the window is now; `saved` is the frame
/// remembered as the one before zooming. A toggle mid-zoom reverses to
/// where that zoom came from, and a reversal of a zoom-out puts the frame
/// it was heading for back, or the next zoom would find nothing saved and
/// fall to AppKit's native, stretching one.
fn zoom_target(
    running: Option<(NSRect, NSRect)>,
    frame: NSRect,
    working_area: NSRect,
    saved: Option<NSRect>,
) -> (ZoomOutcome, Option<NSRect>) {
    match running {
        Some((start, target)) if same_frame(target, working_area) => {
            (ZoomOutcome::Step(start), saved)
        }
        Some((_, target)) => (ZoomOutcome::Step(working_area), Some(target)),
        None if same_frame(frame, working_area) => match saved {
            Some(saved) => (ZoomOutcome::Step(saved), None),
            None => (ZoomOutcome::Native, None),
        },
        None => (ZoomOutcome::Step(working_area), Some(frame)),
    }
}

/// Toggles the zoom of the key window between the screen's working area
/// and the frame it had before. The Window > Zoom menu item keeps the
/// native zoom; one it started is undone by the native one too.
pub fn toggle() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = NSApplication::sharedApplication(mtm).keyWindow() else {
        return;
    };
    let Some(screen) = window.screen() else {
        return;
    };
    let working_area = screen.visibleFrame();
    let frame = window.frame();
    // A zoom in flight gives up its place before anything is asked of
    // AppKit: its calls can run delegates synchronously, and nothing of
    // ours may still be borrowed when they do.
    let running = ZOOM.with(|zoom| zoom.borrow_mut().take()).map(|zoom| {
        zoom.timer.invalidate();
        (zoom.start, zoom.target)
    });
    let (outcome, remember) = zoom_target(
        running,
        frame,
        working_area,
        RESTORE_FRAME.with(|saved| saved.get()),
    );
    RESTORE_FRAME.with(|saved| saved.set(remember));
    let target = match outcome {
        ZoomOutcome::Step(target) => target,
        // A zoom started outside this module (the Window > Zoom menu
        // item), which remembers its own frame to restore; AppKit's is
        // better than a guess.
        ZoomOutcome::Native => {
            window.zoom(None);
            return;
        }
    };
    // Reduced motion asks for the destination, not the journey; so does an
    // AppKit that reports no time for the change.
    let duration = window.animationResizeTime(target);
    if NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() || duration <= 0.0 {
        window.setFrame_display(target, true);
        return;
    }
    let handler: Retained<ZoomTick> = unsafe { objc2::msg_send![mtm.alloc::<ZoomTick>(), init] };
    let timer = unsafe {
        NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            STEP.as_secs_f64(),
            &handler,
            sel!(tick:),
            None,
            true,
        )
    };
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    ZOOM.with(|zoom| {
        *zoom.borrow_mut() = Some(Zoom {
            window: window.clone(),
            start: frame,
            target,
            started: Instant::now(),
            duration,
            timer,
        });
    });
}

/// Moves the running zoom one step, and finishes it on the last one.
fn step() {
    let Some(zoom) = ZOOM.with(|cell| cell.borrow().clone()) else {
        return;
    };
    let progress = (zoom.started.elapsed().as_secs_f64() / zoom.duration).min(1.0);
    if progress >= 1.0 {
        // The zoom leaves before its last resize: nothing of it may still
        // be held while AppKit draws, which can run delegates.
        zoom.timer.invalidate();
        ZOOM.with(|cell| *cell.borrow_mut() = None);
    }
    let frame = eased_rect(zoom.start, zoom.target, eased(progress));
    zoom.window.setFrame_display(frame, true);
}

/// The eased position between `start` and `target` at `progress`, as a
/// frame interpolated part by part.
fn eased_rect(start: NSRect, target: NSRect, progress: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(
            start.origin.x + (target.origin.x - start.origin.x) * progress,
            start.origin.y + (target.origin.y - start.origin.y) * progress,
        ),
        NSSize::new(
            start.size.width + (target.size.width - start.size.width) * progress,
            start.size.height + (target.size.height - start.size.height) * progress,
        ),
    )
}

/// A slow start and end, the feel of the system's own window moves.
fn eased(progress: f64) -> f64 {
    if progress < 0.5 {
        4.0 * progress.powi(3)
    } else {
        1.0 - (-2.0 * progress + 2.0).powi(3) / 2.0
    }
}

/// Whether two frames sit at the same place, within a point.
fn same_frame(a: NSRect, b: NSRect) -> bool {
    (a.origin.x - b.origin.x).abs() <= 1.0
        && (a.origin.y - b.origin.y).abs() <= 1.0
        && (a.size.width - b.size.width).abs() <= 1.0
        && (a.size.height - b.size.height).abs() <= 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::{NSPoint, NSSize};

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    #[test]
    fn the_working_area_is_recognized_within_a_point() {
        assert!(same_frame(
            rect(0.0, 0.0, 1512.0, 943.0),
            rect(0.0, 0.0, 1512.0, 943.0)
        ));
        assert!(same_frame(
            rect(0.0, 0.0, 1512.0, 943.0),
            rect(0.5, 0.4, 1511.6, 943.4)
        ));
        assert!(!same_frame(
            rect(0.0, 0.0, 1512.0, 943.0),
            rect(0.0, 25.0, 1512.0, 918.0)
        ));
        assert!(!same_frame(
            rect(0.0, 0.0, 1512.0, 943.0),
            rect(20.0, 0.0, 1512.0, 943.0)
        ));
    }

    #[test]
    fn easing_starts_still_ends_still_and_stays_between() {
        assert_eq!(eased(0.0), 0.0);
        assert_eq!(eased(1.0), 1.0);
        assert_eq!(eased(0.5), 0.5);
        for progress in [0.1, 0.25, 0.4, 0.6, 0.75, 0.9] {
            let eased = eased(progress);
            assert!((0.0..=1.0).contains(&eased), "{eased} at {progress}");
        }
    }

    #[test]
    fn an_eased_frame_runs_from_start_to_target() {
        let start = rect(100.0, 80.0, 640.0, 480.0);
        let target = rect(0.0, 25.0, 1512.0, 918.0);
        assert_eq!(eased_rect(start, target, 0.0), start);
        assert_eq!(eased_rect(start, target, 1.0), target);
        assert_eq!(
            eased_rect(start, target, 0.5),
            rect(
                (100.0 + 0.0) / 2.0,
                (80.0 + 25.0) / 2.0,
                (640.0 + 1512.0) / 2.0,
                (480.0 + 918.0) / 2.0
            )
        );
    }

    #[test]
    fn a_fresh_zoom_remembers_the_frame_it_leaves() {
        let frame = rect(100.0, 80.0, 640.0, 480.0);
        let area = rect(0.0, 25.0, 1512.0, 918.0);
        let (outcome, remember) = zoom_target(None, frame, area, None);
        assert!(matches!(outcome, ZoomOutcome::Step(target) if same_frame(target, area)));
        assert_eq!(remember, Some(frame));
    }

    #[test]
    fn a_zoomed_window_steps_back_to_what_it_remembered() {
        let saved = rect(100.0, 80.0, 640.0, 480.0);
        let area = rect(0.0, 25.0, 1512.0, 918.0);
        let (outcome, remember) = zoom_target(None, area, area, Some(saved));
        assert!(matches!(outcome, ZoomOutcome::Step(target) if same_frame(target, saved)));
        assert_eq!(remember, None);
    }

    #[test]
    fn a_zoom_the_window_came_by_natively_stays_native() {
        let area = rect(0.0, 25.0, 1512.0, 918.0);
        let (outcome, _) = zoom_target(None, area, area, None);
        assert!(matches!(outcome, ZoomOutcome::Native));
    }

    #[test]
    fn reversing_a_zoom_returns_to_where_it_came_from() {
        let start = rect(100.0, 80.0, 640.0, 480.0);
        let area = rect(0.0, 25.0, 1512.0, 918.0);
        let (outcome, remember) = zoom_target(Some((start, area)), start, area, Some(start));
        assert!(matches!(outcome, ZoomOutcome::Step(target) if same_frame(target, start)));
        assert_eq!(remember, Some(start));
    }

    #[test]
    fn reversing_a_restore_keeps_the_frame_to_return_to() {
        let saved = rect(100.0, 80.0, 640.0, 480.0);
        let area = rect(0.0, 25.0, 1512.0, 918.0);
        let (outcome, remember) = zoom_target(Some((area, saved)), area, area, None);
        assert!(matches!(outcome, ZoomOutcome::Step(target) if same_frame(target, area)));
        assert_eq!(remember, Some(saved));
    }
}
