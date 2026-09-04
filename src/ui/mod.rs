//! Window layout: panels, overlays, keyboard shortcuts.

pub mod artist;
pub mod collection;
mod devices;
mod dialogs;
pub mod home;
mod keys;
pub mod library;
pub mod login;
mod lyrics;
pub mod player_bar;
pub mod queue;
pub mod search;
pub mod settings;
pub mod show;
pub mod sidebar;
pub mod topbar;
pub mod widgets;
pub mod winamp;

use egui::{Align2, Color32, CornerRadius, Frame, Margin, Rect, Stroke, vec2};

use crate::api::models::pick_image;
use crate::app::App;
use crate::backend::AuthStatus;
use crate::model::{Action, Page, ToastKind};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let ctx = &ctx;
    keys::handle(app, ctx);
    for path in winamp::dropped_skins(ctx) {
        app.actions.push(Action::InstallSkin(path));
    }
    let signed_in = app.is_connected() && app.user.is_some();
    let connecting = matches!(app.auth, AuthStatus::Connecting | AuthStatus::Starting)
        || (app.is_connected() && app.user.is_none());
    if !signed_in {
        login::show(app, ui, connecting);
        toasts(app, ctx, 20.0);
        window_controls(ui, &app.palette);
        window_resize(ui);
        return;
    }
    player_bar::show(app, ui);
    if app.settings.sidebar_visible {
        sidebar::show(app, ui);
    }
    if app.show_queue_panel {
        queue::side_panel(app, ui);
    }
    if app.show_lyrics_panel {
        lyrics::side_panel(app, ui);
    }
    central(app, ui);
    devices::popup(app, ctx);
    dialogs::show(app, ctx);
    widgets::drag_ghost(ctx, &app.palette);
    toasts(app, ctx, theme::PLAYER_BAR_HEIGHT + 16.0);
    window_controls(ui, &app.palette);
    window_resize(ui);
}

fn page_tint(app: &mut App) -> Option<Color32> {
    let page = app.page().clone();
    let image = match &page {
        Page::Playlist(id) => app
            .playlist_pages
            .get(id)
            .and_then(|page| page.playlist.get())
            .and_then(|playlist| pick_image(&playlist.images, 300))
            .map(str::to_string),
        Page::Album(id) => app
            .album_pages
            .get(id)
            .and_then(|page| page.album.get())
            .and_then(|album| pick_image(&album.images, 300))
            .map(str::to_string),
        Page::Artist(id) => app
            .artist_pages
            .get(id)
            .and_then(|page| page.artist.get())
            .and_then(|artist| pick_image(&artist.images, 300))
            .map(str::to_string),
        Page::Show(id) => app
            .show_pages
            .get(id)
            .and_then(|page| page.show.get())
            .and_then(|show| pick_image(&show.images, 300))
            .map(str::to_string),
        Page::LikedSongs => return Some(Color32::from_rgb(0x50, 0x38, 0xc8)),
        _ => None,
    };
    if !app.settings.accent_from_art && image.is_some() {
        return None;
    }
    match image {
        Some(url) => app.tint_for(Some(&url)),
        None => app.now_playing_tint(),
    }
}

fn central(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let tint = page_tint(app);
    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.window))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            if let Some(tint) = tint {
                let strength = if matches!(
                    app.page(),
                    Page::Home | Page::Search | Page::Settings | Page::Queue
                ) {
                    0.45
                } else {
                    0.85
                };
                let top = blend(palette.window, tint, strength);
                let header = Rect::from_min_size(rect.min, vec2(rect.width(), 340.0));
                widgets::paint_vertical_gradient(ui, header, top, palette.window);
            }
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            topbar::show(app, ui);
            let page = app.page().clone();
            egui::ScrollArea::vertical()
                .id_salt(("page", page.encode()))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    Frame::new()
                        .inner_margin(Margin {
                            left: widgets::PAGE_PADDING as i8,
                            right: widgets::PAGE_PADDING as i8,
                            top: 4,
                            bottom: 48,
                        })
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            match page {
                                Page::Home => home::show(app, ui),
                                Page::TopSongs => collection::top_songs(app, ui),
                                Page::Search => search::show(app, ui),
                                Page::LikedSongs => collection::liked(app, ui),
                                Page::Albums | Page::Artists | Page::Podcasts | Page::Episodes => {
                                    library::show(app, ui, page)
                                }
                                Page::Playlist(id) => collection::playlist(app, ui, &id),
                                Page::Album(id) => collection::album(app, ui, &id),
                                Page::Artist(id) => artist::show(app, ui, &id),
                                Page::Show(id) => show::show(app, ui, &id),
                                Page::Queue => queue::page(app, ui),
                                Page::Settings => settings::show(app, ui),
                            }
                        });
                });
        });
}

/// What a press on the title bar does, decided on the press itself the way
/// AppKit decides at mouse-down.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TitlebarPress {
    /// Drag the window.
    Drag,
    /// The press completes a double-click: zoom the window.
    Zoom,
}

/// Reads a title-bar press and returns what it does and the press the next
/// one should pair with. egui counts double-clicks on release, but on macOS
/// the press starts an AppKit drag session that can swallow the release, so
/// the pairing runs press to press instead, like AppKit's own click counting,
/// and the zoom lands on the second press rather than on its release. A
/// completed pair is spent: a third quick press drags again rather than
/// zooming a second time.
fn titlebar_press(
    now: f64,
    pos: egui::Pos2,
    previous: Option<(f64, egui::Pos2)>,
    double_click_delay: f64,
    double_click_dist: f32,
) -> (TitlebarPress, Option<(f64, egui::Pos2)>) {
    match previous {
        Some((then, at))
            if now - then <= double_click_delay && at.distance(pos) <= double_click_dist =>
        {
            (TitlebarPress::Zoom, None)
        }
        _ => (TitlebarPress::Drag, Some((now, pos))),
    }
}

/// Remembers a title-bar press for the next one to pair with. The store is
/// keyed by type as well as id, so a spent pair removes the entry instead of
/// writing an `Option` a differently typed read would never find.
fn remember_titlebar_press(ctx: &egui::Context, id: egui::Id, press: Option<(f64, egui::Pos2)>) {
    ctx.data_mut(|data| match press {
        Some(at) => {
            data.insert_temp(id, at);
        }
        None => data.remove::<(f64, egui::Pos2)>(id),
    });
}

/// The press the next title-bar press pairs with, if one is remembered.
fn last_titlebar_press(ctx: &egui::Context, id: egui::Id) -> Option<(f64, egui::Pos2)> {
    ctx.data(|data| data.get_temp(id))
}

/// Zooms the window. On macOS the zoom steps the window itself, one plain
/// resize at a time, because the toolkit's command animates on the window
/// server, from a snapshot of the window, which stretches the interface
/// (src/mac_window.rs); everywhere else the toolkit's maximize does the
/// job, restoring the size and position the window had before.
fn zoom_window(_ctx: &egui::Context) {
    #[cfg(target_os = "macos")]
    {
        crate::mac_window::toggle();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let maximized = _ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        _ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
}

/// Makes `rect` the window's title bar: a press drags it, a double-click
/// zooms it. Register it before child widgets so they keep their clicks.
pub fn titlebar_interaction(ui: &mut egui::Ui, rect: egui::Rect) {
    let app_owns_titlebar = cfg!(any(target_os = "macos", windows));
    let fullscreen = ui
        .ctx()
        .input(|input| input.viewport().fullscreen.unwrap_or(false));
    if !app_owns_titlebar || fullscreen {
        return;
    }
    let response = ui.interact(
        rect,
        ui.id().with("titlebar"),
        egui::Sense::click_and_drag(),
    );
    let gesture = if cfg!(target_os = "macos") {
        // macOS needs the live mouse-down event rather than egui's drag threshold.
        if response.is_pointer_button_down_on() && ui.input(|input| input.pointer.primary_pressed())
        {
            let Some(pos) = ui.input(|input| input.pointer.interact_pos()) else {
                return;
            };
            let id = ui.id().with("titlebar-press");
            let previous = last_titlebar_press(ui.ctx(), id);
            let (double_click_delay, double_click_dist) = ui.ctx().options(|options| {
                (
                    options.input_options.max_double_click_delay,
                    options.input_options.max_click_dist,
                )
            });
            let now = ui.input(|input| input.time);
            let (gesture, remember) =
                titlebar_press(now, pos, previous, double_click_delay, double_click_dist);
            remember_titlebar_press(ui.ctx(), id, remember);
            Some(gesture)
        } else {
            None
        }
    } else if response.double_clicked() {
        Some(TitlebarPress::Zoom)
    } else if response.drag_started() {
        Some(TitlebarPress::Drag)
    } else {
        None
    };
    match gesture {
        Some(TitlebarPress::Drag) => {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        Some(TitlebarPress::Zoom) => zoom_window(ui.ctx()),
        None => {}
    }
}

const WINDOW_RESIZE_BORDER: f32 = 5.0;
const WINDOW_RESIZE_CORNER: f32 = 10.0;
const WINDOWS_WINDOW_CONTROLS_WIDTH: f32 = 3.0 * 36.0 + WINDOW_RESIZE_BORDER;
const WINDOWS_WINDOW_CONTROLS_HEIGHT: f32 = 36.0 + WINDOW_RESIZE_BORDER;
// The 760-point minimum with the default 250-point sidebar leaves 510 points.
const WINDOWS_MIN_INLINE_TOPBAR_WIDTH: f32 = 510.0 + WINDOWS_WINDOW_CONTROLS_WIDTH;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WindowControlsReservation {
    pub topbar_width: f32,
    pub topbar_top: f32,
    pub queue_top: f32,
    pub lyrics_top: f32,
}

const fn windows_chrome_visible(on_windows: bool, fullscreen: bool) -> bool {
    on_windows && !fullscreen
}

fn windows_chrome_visible_here(ctx: &egui::Context) -> bool {
    let fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
    windows_chrome_visible(cfg!(windows), fullscreen)
}

const fn windows_controls_reservation(
    on_windows: bool,
    fullscreen: bool,
    queue: bool,
    lyrics: bool,
    topbar_width: f32,
) -> WindowControlsReservation {
    let mut space = WindowControlsReservation {
        topbar_width: 0.0,
        topbar_top: 0.0,
        queue_top: 0.0,
        lyrics_top: 0.0,
    };
    if windows_chrome_visible(on_windows, fullscreen) {
        if queue {
            space.queue_top = WINDOWS_WINDOW_CONTROLS_HEIGHT;
        } else if lyrics {
            space.lyrics_top = WINDOWS_WINDOW_CONTROLS_HEIGHT;
        } else if topbar_width < WINDOWS_MIN_INLINE_TOPBAR_WIDTH {
            space.topbar_top = WINDOWS_WINDOW_CONTROLS_HEIGHT;
        } else {
            space.topbar_width = WINDOWS_WINDOW_CONTROLS_WIDTH;
        }
    }
    space
}

pub(super) fn window_controls_reservation(
    ctx: &egui::Context,
    queue: bool,
    lyrics: bool,
    topbar_width: f32,
) -> WindowControlsReservation {
    let fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
    windows_controls_reservation(cfg!(windows), fullscreen, queue, lyrics, topbar_width)
}

/// Draws the Windows caption controls over the outermost top-right header.
pub fn window_controls(ui: &mut egui::Ui, palette: &theme::Palette) {
    if !windows_chrome_visible_here(ui.ctx()) {
        return;
    }
    let maximized = ui
        .ctx()
        .input(|input| input.viewport().maximized.unwrap_or(false));
    egui::Area::new(egui::Id::new("windows-window-controls"))
        .anchor(
            Align2::RIGHT_TOP,
            vec2(-WINDOW_RESIZE_BORDER, WINDOW_RESIZE_BORDER),
        )
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                for (icon, tooltip, command) in [
                    (
                        Icon::Minus,
                        "Minimize",
                        egui::ViewportCommand::Minimized(true),
                    ),
                    (
                        if maximized { Icon::Copy } else { Icon::Square },
                        if maximized { "Restore" } else { "Maximize" },
                        egui::ViewportCommand::Maximized(!maximized),
                    ),
                    (Icon::X, "Close", egui::ViewportCommand::Close),
                ] {
                    let image = icon.image(palette.secondary, 14.0).alt_text(tooltip);
                    let button = egui::Button::image(image).frame_when_inactive(false);
                    if ui
                        .add_sized(egui::Vec2::splat(36.0), button)
                        .on_hover_text(tooltip)
                        .clicked()
                    {
                        ui.ctx().send_viewport_cmd(command);
                    }
                }
            });
        });
}

fn window_resize(ui: &mut egui::Ui) {
    let (fullscreen, maximized) = ui.ctx().input(|input| {
        (
            input.viewport().fullscreen.unwrap_or(false),
            input.viewport().maximized.unwrap_or(false),
        )
    });
    if !window_resize_enabled(cfg!(windows), fullscreen, maximized) {
        return;
    }

    let Some(position) = ui.input(|input| input.pointer.hover_pos()) else {
        return;
    };
    let Some(direction) = resize_direction(ui.ctx().content_rect(), position) else {
        return;
    };
    ui.ctx().set_cursor_icon(direction.1);
    if ui.input(|input| input.pointer.primary_pressed()) {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction.0));
    }
}

const fn window_resize_enabled(on_windows: bool, fullscreen: bool, maximized: bool) -> bool {
    on_windows && !fullscreen && !maximized
}

fn resize_direction(
    window: Rect,
    position: egui::Pos2,
) -> Option<(egui::ResizeDirection, egui::CursorIcon)> {
    if !window.contains(position) {
        return None;
    }
    let [left, right, top, bottom] = [
        position.x - window.left(),
        window.right() - position.x,
        position.y - window.top(),
        window.bottom() - position.y,
    ];
    let mut x = i8::from(right <= WINDOW_RESIZE_BORDER) - i8::from(left <= WINDOW_RESIZE_BORDER);
    let mut y = i8::from(bottom <= WINDOW_RESIZE_BORDER) - i8::from(top <= WINDOW_RESIZE_BORDER);
    if y != 0 {
        x = i8::from(right <= WINDOW_RESIZE_CORNER) - i8::from(left <= WINDOW_RESIZE_CORNER);
    }
    if x != 0 {
        y = i8::from(bottom <= WINDOW_RESIZE_CORNER) - i8::from(top <= WINDOW_RESIZE_CORNER);
    }
    use egui::{CursorIcon as C, ResizeDirection as D};
    match (x, y) {
        (-1, -1) => Some((D::NorthWest, C::ResizeNwSe)),
        (1, -1) => Some((D::NorthEast, C::ResizeNeSw)),
        (-1, 1) => Some((D::SouthWest, C::ResizeNeSw)),
        (1, 1) => Some((D::SouthEast, C::ResizeNwSe)),
        (-1, 0) => Some((D::West, C::ResizeHorizontal)),
        (1, 0) => Some((D::East, C::ResizeHorizontal)),
        (0, -1) => Some((D::North, C::ResizeVertical)),
        (0, 1) => Some((D::South, C::ResizeVertical)),
        _ => None,
    }
}

pub fn blend(base: Color32, tint: Color32, amount: f32) -> Color32 {
    let a = egui::Rgba::from(base);
    let b = egui::Rgba::from(tint);
    let mixed = a * (1.0 - amount) + b * amount;
    let mut color = Color32::from(mixed);
    color[3] = 255;
    color
}

fn toasts(app: &mut App, ctx: &egui::Context, bottom_offset: f32) {
    if app.toasts.is_empty() {
        return;
    }
    let palette = app.palette;
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -bottom_offset))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            for toast in &app.toasts {
                let age = toast.created.elapsed().as_secs_f32();
                let alpha = if age < 0.15 {
                    age / 0.15
                } else if age > 2.8 {
                    ((3.2 - age) / 0.4).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                ui.set_opacity(alpha);
                Frame::new()
                    .fill(palette.overlay)
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(theme::RADIUS))
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: palette.shadow,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (icon, color) = match toast.kind {
                                ToastKind::Info => (Icon::CircleCheck, palette.accent),
                                ToastKind::Error => (Icon::CircleAlert, palette.danger),
                            };
                            theme::icon(ui, icon, 16.0, color);
                            // Laid out at its own width. The area
                            // remembers its size, so after a short toast a
                            // label left to wrap at the area's width broke
                            // long messages on every word.
                            let galley = ui.painter().layout(
                                toast.message.clone(),
                                theme::medium(13.5),
                                palette.text,
                                280.0,
                            );
                            ui.add(egui::Label::new(galley));
                        });
                    });
            }
        });
}

#[cfg(test)]
mod titlebar_press_tests {
    use super::*;

    /// egui's own limits: a double-click within 0.3 s and 6 points.
    const DELAY: f64 = 0.3;
    const DIST: f32 = 6.0;

    fn press(
        now: f64,
        pos: egui::Pos2,
        previous: Option<(f64, egui::Pos2)>,
    ) -> (TitlebarPress, Option<(f64, egui::Pos2)>) {
        titlebar_press(now, pos, previous, DELAY, DIST)
    }

    #[test]
    fn a_press_without_a_partner_drags_and_is_remembered() {
        let (gesture, remember) = press(1.0, egui::pos2(10.0, 5.0), None);
        assert_eq!(gesture, TitlebarPress::Drag);
        assert_eq!(remember, Some((1.0, egui::pos2(10.0, 5.0))));
    }

    #[test]
    fn the_second_press_of_a_quick_pair_in_place_zooms_and_spends_the_pair() {
        let (gesture, remember) = press(
            1.1,
            egui::pos2(12.0, 6.0),
            Some((1.0, egui::pos2(10.0, 5.0))),
        );
        assert_eq!(gesture, TitlebarPress::Zoom);
        assert_eq!(remember, None);
    }

    #[test]
    fn a_press_after_the_double_click_window_drags() {
        let (gesture, _) = press(
            1.4,
            egui::pos2(10.0, 5.0),
            Some((1.0, egui::pos2(10.0, 5.0))),
        );
        assert_eq!(gesture, TitlebarPress::Drag);
    }

    #[test]
    fn a_press_far_from_its_partner_drags() {
        let (gesture, _) = press(
            1.1,
            egui::pos2(100.0, 5.0),
            Some((1.0, egui::pos2(10.0, 5.0))),
        );
        assert_eq!(gesture, TitlebarPress::Drag);
    }

    /// The store and its reader must agree on the type they keep, or a
    /// written press is never read back and no press ever pairs.
    #[test]
    fn a_remembered_press_round_trips_and_a_spent_pair_is_forgotten() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("titlebar-press-test");
        remember_titlebar_press(&ctx, id, Some((1.0, egui::pos2(10.0, 5.0))));
        assert_eq!(
            last_titlebar_press(&ctx, id),
            Some((1.0, egui::pos2(10.0, 5.0)))
        );
        remember_titlebar_press(&ctx, id, None);
        assert_eq!(last_titlebar_press(&ctx, id), None);
    }
}

#[cfg(test)]
mod window_chrome_tests {
    use super::*;

    #[test]
    fn chrome_visibility_matches_window_state() {
        assert!(windows_chrome_visible(true, false));
        assert!(!windows_chrome_visible(true, true));
        assert!(!windows_chrome_visible(false, false));
        assert!(window_resize_enabled(true, false, false));
        assert!(!window_resize_enabled(true, false, true));
        assert!(!window_resize_enabled(true, true, false));
        assert!(!window_resize_enabled(false, false, false));
    }

    #[test]
    fn caption_space_belongs_to_the_outermost_header() {
        let values = |queue, lyrics| {
            let space = windows_controls_reservation(true, false, queue, lyrics, f32::INFINITY);
            [
                space.topbar_width,
                space.topbar_top,
                space.queue_top,
                space.lyrics_top,
            ]
        };
        assert_eq!(
            values(false, false),
            [WINDOWS_WINDOW_CONTROLS_WIDTH, 0.0, 0.0, 0.0]
        );
        assert_eq!(
            values(true, false),
            [0.0, 0.0, WINDOWS_WINDOW_CONTROLS_HEIGHT, 0.0]
        );
        assert_eq!(
            values(false, true),
            [0.0, 0.0, 0.0, WINDOWS_WINDOW_CONTROLS_HEIGHT]
        );
        assert_eq!(
            values(true, true),
            [0.0, 0.0, WINDOWS_WINDOW_CONTROLS_HEIGHT, 0.0]
        );
        assert_eq!(
            windows_controls_reservation(true, true, true, true, f32::INFINITY),
            WindowControlsReservation {
                topbar_width: 0.0,
                topbar_top: 0.0,
                queue_top: 0.0,
                lyrics_top: 0.0,
            }
        );
    }

    #[test]
    fn minimum_windows_window_stacks_caption_space_above_the_topbar() {
        let available = 760.0 - 250.0;
        let space = windows_controls_reservation(true, false, false, false, available);
        assert_eq!(space.topbar_width, 0.0);
        assert_eq!(space.topbar_top, WINDOWS_WINDOW_CONTROLS_HEIGHT);

        let inline = windows_controls_reservation(
            true,
            false,
            false,
            false,
            WINDOWS_MIN_INLINE_TOPBAR_WIDTH,
        );
        assert_eq!(inline.topbar_width, WINDOWS_WINDOW_CONTROLS_WIDTH);
        assert_eq!(inline.topbar_top, 0.0);
    }

    #[test]
    fn resize_hit_test_covers_edges_and_corners() {
        use egui::ResizeDirection as D;

        let window = Rect::from_min_max(egui::pos2(20.0, 30.0), egui::pos2(120.0, 110.0));
        for (position, expected) in [
            (egui::pos2(21.0, 31.0), Some(D::NorthWest)),
            (egui::pos2(70.0, 31.0), Some(D::North)),
            (egui::pos2(119.0, 31.0), Some(D::NorthEast)),
            (egui::pos2(21.0, 70.0), Some(D::West)),
            (egui::pos2(70.0, 70.0), None),
            (egui::pos2(119.0, 70.0), Some(D::East)),
            (egui::pos2(21.0, 109.0), Some(D::SouthWest)),
            (egui::pos2(70.0, 109.0), Some(D::South)),
            (egui::pos2(119.0, 109.0), Some(D::SouthEast)),
        ] {
            assert_eq!(
                resize_direction(window, position).map(|hit| hit.0),
                expected
            );
        }
    }
}
