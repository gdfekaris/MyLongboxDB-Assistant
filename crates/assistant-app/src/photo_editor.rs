//! The photo editor: turn, straighten, and crop a cover photo before it
//! is looked up and exported. It works on a small preview; the same edit
//! is then applied to the full photo away from the window.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use assistant_media::{Crop, Edit, MAX_STRAIGHTEN, RgbImage, apply_edit};
use gtk::{cairo, gdk, glib};

use crate::i18n::gettext;
use crate::photos;

/// How close, in pixels, the pointer must be to grab an edge or corner.
const REACH: f64 = 14.0;

/// How far an arrow key moves the selected part of the crop, in pixels
/// of the picture as shown, and how far with Shift held.
const NUDGE: f64 = 1.0;
const BIG_NUDGE: f64 = 10.0;

/// Which edges of the crop a drag moves; all four moves the whole crop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Grip {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
}

impl Grip {
    const MOVE: Self = Self {
        left: true,
        top: true,
        right: true,
        bottom: true,
    };

    /// What Tab steps through, in order: the whole crop, then each edge.
    const CYCLE: [Self; 5] = [
        Self::MOVE,
        Self::edge(true, false, false, false),
        Self::edge(false, true, false, false),
        Self::edge(false, false, true, false),
        Self::edge(false, false, false, true),
    ];

    const fn edge(left: bool, top: bool, right: bool, bottom: bool) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    /// The part Tab selects after this one, or `None` past the last,
    /// when focus moves on out of the crop. A corner, picked with the
    /// pointer, counts as the whole crop.
    fn next(self, back: bool) -> Option<Self> {
        let at = Self::CYCLE.iter().position(|g| *g == self).unwrap_or(0);
        let to = if back { at.checked_sub(1)? } else { at + 1 };
        Self::CYCLE.get(to).copied()
    }

    fn cursor(self) -> &'static str {
        match (self.left || self.right, self.top || self.bottom) {
            _ if self == Self::MOVE => "move",
            (true, true) if (self.left && self.top) || (self.right && self.bottom) => "nwse-resize",
            (true, true) => "nesw-resize",
            (true, false) => "ew-resize",
            _ => "ns-resize",
        }
    }
}

/// Where the picture sits in the editing area: its offset and scale.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    x: f64,
    y: f64,
    scale: f64,
    width: f64,
    height: f64,
}

impl Layout {
    /// The picture shown whole and centred, as `gtk::Picture` shows it.
    fn new(area: (f64, f64), frame: (u32, u32)) -> Self {
        let (fw, fh) = (f64::from(frame.0.max(1)), f64::from(frame.1.max(1)));
        let scale = (area.0 / fw).min(area.1 / fh).max(f64::MIN_POSITIVE);
        let (width, height) = (fw * scale, fh * scale);
        Self {
            x: (area.0 - width) / 2.0,
            y: (area.1 - height) / 2.0,
            scale,
            width,
            height,
        }
    }

    /// The crop's edges in the area's coordinates.
    fn edges(&self, crop: Crop) -> (f64, f64, f64, f64) {
        (
            self.x + f64::from(crop.left) * self.width,
            self.y + f64::from(crop.top) * self.height,
            self.x + f64::from(crop.right) * self.width,
            self.y + f64::from(crop.bottom) * self.height,
        )
    }

    /// What a press at `(x, y)` would grab: an edge or corner within
    /// reach, else the whole crop if inside it.
    fn grip(&self, crop: Crop, x: f64, y: f64) -> Option<Grip> {
        let (l, t, r, b) = self.edges(crop);
        let within = |v: f64, from: f64, to: f64| v >= from - REACH && v <= to + REACH;
        if !within(x, l, r) || !within(y, t, b) {
            return None;
        }
        let near = |v: f64, edge: f64| (v - edge).abs() <= REACH;
        let grip = Grip {
            left: near(x, l),
            top: near(y, t),
            right: near(x, r) && !near(x, l),
            bottom: near(y, b) && !near(y, t),
        };
        if grip.left || grip.top || grip.right || grip.bottom {
            Some(grip)
        } else if x > l && x < r && y > t && y < b {
            Some(Grip::MOVE)
        } else {
            None
        }
    }

    /// The crop after dragging `grip` by `(dx, dy)` area pixels from
    /// `start`.
    fn dragged(&self, start: Crop, grip: Grip, dx: f64, dy: f64) -> Crop {
        // Shares of the picture are small numbers; f32 holds them well.
        let sx = (dx / self.width) as f32;
        let sy = (dy / self.height) as f32;
        if grip == Grip::MOVE {
            let sx = sx.clamp(-start.left, 1.0 - start.right);
            let sy = sy.clamp(-start.top, 1.0 - start.bottom);
            return Crop {
                left: start.left + sx,
                top: start.top + sy,
                right: start.right + sx,
                bottom: start.bottom + sy,
            };
        }
        let mut crop = start;
        if grip.left {
            crop.left = (start.left + sx).min(start.right - assistant_media::MIN_CROP);
        }
        if grip.right {
            crop.right = (start.right + sx).max(start.left + assistant_media::MIN_CROP);
        }
        if grip.top {
            crop.top = (start.top + sy).min(start.bottom - assistant_media::MIN_CROP);
        }
        if grip.bottom {
            crop.bottom = (start.bottom + sy).max(start.top + assistant_media::MIN_CROP);
        }
        crop.clamped()
    }
}

/// What one of the editor's buttons does.
type Action = fn(&Editor);

struct Editor {
    preview: RgbImage,
    edit: Cell<Edit>,
    /// The size of the turned preview, which the crop is a share of.
    frame: Cell<(u32, u32)>,
    picture: gtk::Picture,
    area: gtk::DrawingArea,
    straighten: gtk::Scale,
    drag: Cell<Option<(Grip, Crop)>>,
    /// What the arrow keys move: the part of the crop last grabbed or
    /// picked with Tab.
    selected: Cell<Grip>,
    /// Whether a redraw is already waiting to run.
    redraw_waiting: Cell<bool>,
}

impl Editor {
    /// Redraws the preview turned and straightened, uncropped; the crop
    /// is drawn over it.
    fn refresh(&self) {
        let whole = Edit {
            crop: Crop::FULL,
            ..self.edit.get()
        };
        let frame = apply_edit(&self.preview, &whole);
        self.frame.set(frame.dimensions());
        self.picture
            .set_paintable(photos::texture_of(&frame).as_ref());
        self.area.queue_draw();
    }

    fn layout(&self) -> Layout {
        Layout::new(
            (f64::from(self.area.width()), f64::from(self.area.height())),
            self.frame.get(),
        )
    }

    fn change(&self, f: impl FnOnce(&mut Edit)) {
        let mut edit = self.edit.get();
        f(&mut edit);
        self.edit.set(edit);
        self.refresh();
    }

    /// Like [`Self::change`], for the straighten slider: a redraw takes
    /// tens of milliseconds and a drag sends values faster than that, so
    /// the values that arrive meanwhile are drawn once, together.
    fn change_soon(self: &Rc<Self>, f: impl FnOnce(&mut Edit)) {
        let mut edit = self.edit.get();
        f(&mut edit);
        self.edit.set(edit);
        if !self.redraw_waiting.replace(true) {
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(e) = weak.upgrade() {
                    e.redraw_waiting.set(false);
                    e.refresh();
                }
            });
        }
    }

    fn draw(&self, cr: &cairo::Context) {
        let layout = self.layout();
        let (l, t, r, b) = layout.edges(self.edit.get().crop);
        // Dim what the crop leaves out.
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.rectangle(layout.x, layout.y, layout.width, layout.height);
        cr.rectangle(l, t, r - l, b - t);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
        let _ = cr.fill();
        // Thirds, to help line the cover up while straightening.
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
        cr.set_line_width(1.0);
        for n in [1.0, 2.0] {
            let x = l + (r - l) * n / 3.0;
            let y = t + (b - t) * n / 3.0;
            cr.move_to(x, t);
            cr.line_to(x, b);
            cr.move_to(l, y);
            cr.line_to(r, y);
        }
        let _ = cr.stroke();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(2.0);
        cr.rectangle(l, t, r - l, b - t);
        let _ = cr.stroke();
        for (x, y) in [(l, t), (r, t), (l, b), (r, b)] {
            cr.rectangle(x - 5.0, y - 5.0, 10.0, 10.0);
        }
        let _ = cr.fill();
        // While the crop has keyboard focus, mark what the arrows move.
        if self.area.is_focus() {
            let g = self.selected.get();
            cr.set_source_rgb(0.21, 0.52, 0.89);
            cr.set_line_width(4.0);
            // Drawn just inside the crop, so an edge on the picture's
            // border is not half cut off.
            let (l, t, r, b) = (l + 2.0, t + 2.0, r - 2.0, b - 2.0);
            if g == Grip::MOVE {
                cr.rectangle(l, t, r - l, b - t);
            } else {
                for (on, (x0, y0, x1, y1)) in [
                    (g.left, (l, t, l, b)),
                    (g.top, (l, t, r, t)),
                    (g.right, (r, t, r, b)),
                    (g.bottom, (l, b, r, b)),
                ] {
                    if on {
                        cr.move_to(x0, y0);
                        cr.line_to(x1, y1);
                    }
                }
            }
            let _ = cr.stroke();
        }
    }

    /// Moves the selected part of the crop by `(dx, dy)` area pixels.
    fn nudge(&self, dx: f64, dy: f64) {
        let mut edit = self.edit.get();
        edit.crop = self
            .layout()
            .dragged(edit.crop, self.selected.get(), dx, dy);
        self.edit.set(edit);
        self.area.queue_draw();
    }
}

/// Opens the editor on `preview`, with the crop at the photo's edges,
/// since no guess at the cover suits every photo: a slabbed book's case
/// and label belong in it, a raw book's table does not. `done` gets the
/// edit when the owner saves; closing the editor otherwise leaves the
/// photo as it was.
pub fn open(
    parent: &impl IsA<gtk::Widget>,
    title: &str,
    preview: RgbImage,
    done: impl Fn(Edit) + 'static,
) {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .can_shrink(true)
        .hexpand(true)
        .vexpand(true)
        .build();
    let area = gtk::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .focusable(true)
        .focus_on_click(true)
        .build();
    // Translators: the frame drawn over a photo to choose what part of it
    // to keep.
    let crop_name = gettext("Crop");
    area.update_property(&[
        gtk::accessible::Property::Label(&crop_name),
        gtk::accessible::Property::Description(&gettext(
            "Arrow keys move the selected edge, or the whole crop; Shift moves it further. \
             Tab selects the next edge.",
        )),
    ]);
    let canvas = gtk::Overlay::builder()
        .child(&picture)
        .margin_top(18)
        .margin_bottom(12)
        .margin_start(18)
        .margin_end(18)
        .build();
    canvas.add_overlay(&area);

    let straighten = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        f64::from(-MAX_STRAIGHTEN),
        f64::from(MAX_STRAIGHTEN),
        0.1,
    );
    straighten.set_value(0.0);
    straighten.set_digits(1);
    // Arrow keys step by a tenth of a degree; rounding keeps the steps
    // from drifting.
    straighten.set_round_digits(1);
    straighten.set_format_value_func(|_, degrees| format!("{degrees:.1}\u{b0}"));
    straighten.set_draw_value(true);
    straighten.set_hexpand(true);
    straighten.add_mark(0.0, gtk::PositionType::Bottom, None);
    // Translators: rotating the photo a few degrees so the cover is level.
    let straighten_name = gettext("Straighten");
    straighten.update_property(&[gtk::accessible::Property::Label(&straighten_name)]);

    let editor = Rc::new(Editor {
        preview,
        edit: Cell::new(Edit::default()),
        frame: Cell::new((1, 1)),
        picture,
        area: area.clone(),
        straighten: straighten.clone(),
        drag: Cell::new(None),
        selected: Cell::new(Grip::MOVE),
        redraw_waiting: Cell::new(false),
    });
    editor.refresh();

    {
        let weak = Rc::downgrade(&editor);
        area.set_draw_func(move |_, cr, _, _| {
            if let Some(editor) = weak.upgrade() {
                editor.draw(cr);
            }
        });
    }
    let drag = gtk::GestureDrag::new();
    {
        let weak = Rc::downgrade(&editor);
        drag.connect_drag_begin(move |_, x, y| {
            if let Some(e) = weak.upgrade() {
                let crop = e.edit.get().crop;
                let grip = e.layout().grip(crop, x, y);
                e.drag.set(grip.map(|g| (g, crop)));
                if let Some(grip) = grip {
                    e.selected.set(grip);
                    e.area.grab_focus();
                    e.area.queue_draw();
                }
            }
        });
    }
    {
        let weak = Rc::downgrade(&editor);
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some(e) = weak.upgrade()
                && let Some((grip, start)) = e.drag.get()
            {
                let crop = e.layout().dragged(start, grip, dx, dy);
                let mut edit = e.edit.get();
                edit.crop = crop;
                e.edit.set(edit);
                e.area.queue_draw();
            }
        });
    }
    {
        let weak = Rc::downgrade(&editor);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(e) = weak.upgrade() {
                e.drag.set(None);
            }
        });
    }
    area.add_controller(drag);
    let motion = gtk::EventControllerMotion::new();
    {
        let weak = Rc::downgrade(&editor);
        motion.connect_motion(move |_, x, y| {
            if let Some(e) = weak.upgrade()
                && e.drag.get().is_none()
            {
                let grip = e.layout().grip(e.edit.get().crop, x, y);
                e.area.set_cursor_from_name(grip.map(Grip::cursor));
            }
        });
    }
    area.add_controller(motion);
    let keys = gtk::EventControllerKey::new();
    {
        let weak = Rc::downgrade(&editor);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(e) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let step = if state.contains(gdk::ModifierType::SHIFT_MASK) {
                BIG_NUDGE
            } else {
                NUDGE
            };
            let (dx, dy) = match key {
                gdk::Key::Left | gdk::Key::KP_Left => (-step, 0.0),
                gdk::Key::Right | gdk::Key::KP_Right => (step, 0.0),
                gdk::Key::Up | gdk::Key::KP_Up => (0.0, -step),
                gdk::Key::Down | gdk::Key::KP_Down => (0.0, step),
                gdk::Key::Tab | gdk::Key::KP_Tab | gdk::Key::ISO_Left_Tab => {
                    let back = key == gdk::Key::ISO_Left_Tab
                        || state.contains(gdk::ModifierType::SHIFT_MASK);
                    // Past either end, Tab leaves the crop as usual.
                    let Some(next) = e.selected.get().next(back) else {
                        return glib::Propagation::Proceed;
                    };
                    e.selected.set(next);
                    e.area.queue_draw();
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            };
            e.nudge(dx, dy);
            glib::Propagation::Stop
        });
    }
    area.add_controller(keys);
    let focus = gtk::EventControllerFocus::new();
    {
        let weak = Rc::downgrade(&editor);
        focus.connect_enter(move |_| {
            if let Some(e) = weak.upgrade() {
                e.area.queue_draw();
            }
        });
    }
    {
        // Coming back with Tab starts again from the whole crop.
        let weak = Rc::downgrade(&editor);
        focus.connect_leave(move |_| {
            if let Some(e) = weak.upgrade() {
                e.selected.set(Grip::MOVE);
                e.area.queue_draw();
            }
        });
    }
    area.add_controller(focus);
    {
        let weak = Rc::downgrade(&editor);
        straighten.connect_value_changed(move |scale| {
            if let Some(e) = weak.upgrade() {
                // The slider's range matches the limit, so this is safe
                // to narrow.
                e.change_soon(|edit| edit.set_straighten(scale.value() as f32));
            }
        });
    }

    let button = |icon: Option<&str>, label: Option<&str>, tip: &str| {
        let b = gtk::Button::builder().tooltip_text(tip).build();
        if let Some(icon) = icon {
            b.set_icon_name(icon);
        }
        if let Some(label) = label {
            b.set_label(label);
            b.set_use_underline(true);
        }
        b
    };
    let left = button(
        Some("object-rotate-left-symbolic"),
        None,
        &gettext("Turn Left"),
    );
    let right = button(
        Some("object-rotate-right-symbolic"),
        None,
        &gettext("Turn Right"),
    );
    let reset = button(None, Some(&gettext("_Reset")), &gettext("Undo All Changes"));
    let actions: [(&gtk::Button, Action); 3] = [
        (&left, |e| e.change(Edit::turn_left)),
        (&right, |e| e.change(Edit::turn_right)),
        (&reset, |e| {
            e.change(|edit| *edit = Edit::default());
            e.straighten.set_value(0.0);
        }),
    ];
    for (b, action) in actions {
        let weak = Rc::downgrade(&editor);
        b.connect_clicked(move |_| {
            if let Some(e) = weak.upgrade() {
                action(&e);
            }
        });
    }
    let straighten_label = gtk::Label::builder()
        .label(&straighten_name)
        .css_classes(["dim-label"])
        .build();
    let controls = gtk::Box::builder()
        .spacing(12)
        .margin_start(18)
        .margin_end(18)
        .margin_bottom(18)
        .build();
    let turns = gtk::Box::builder().css_classes(["linked"]).build();
    turns.append(&left);
    turns.append(&right);
    controls.append(&turns);
    controls.append(&straighten_label);
    controls.append(&straighten);
    controls.append(&reset);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&canvas);
    content.append(&controls);

    let cancel = gtk::Button::with_mnemonic(&gettext("_Cancel"));
    let save = gtk::Button::builder()
        .label(gettext("_Save"))
        .use_underline(true)
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    header.pack_start(&cancel);
    header.pack_end(&save);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));

    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(960)
        .content_height(760)
        .child(&toolbar)
        .default_widget(&save)
        .build();
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    {
        let dialog = dialog.clone();
        let editor = editor.clone();
        let done = RefCell::new(Some(done));
        save.connect_clicked(move |_| {
            if let Some(done) = done.borrow_mut().take() {
                done(editor.edit.get());
            }
            dialog.close();
        });
    }
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        // A 100 × 200 picture in a 400 × 300 area: scaled by 1.5 and
        // centred across.
        Layout::new((400.0, 300.0), (100, 200))
    }

    #[test]
    fn the_picture_is_shown_whole_and_centred() {
        let l = layout();
        assert_eq!(
            l,
            Layout {
                x: 125.0,
                y: 0.0,
                scale: 1.5,
                width: 150.0,
                height: 300.0
            }
        );
        assert_eq!(l.edges(Crop::FULL), (125.0, 0.0, 275.0, 300.0));
    }

    #[test]
    fn presses_grab_corners_edges_or_the_middle() {
        let l = layout();
        let crop = Crop::FULL;
        let at = |x, y| l.grip(crop, x, y);
        assert_eq!(
            at(126.0, 2.0),
            Some(Grip {
                left: true,
                top: true,
                right: false,
                bottom: false
            })
        );
        assert_eq!(at(274.0, 299.0).map(Grip::cursor), Some("nwse-resize"));
        assert_eq!(at(274.0, 1.0).map(Grip::cursor), Some("nesw-resize"));
        assert_eq!(at(130.0, 150.0).map(Grip::cursor), Some("ew-resize"));
        assert_eq!(at(200.0, 296.0).map(Grip::cursor), Some("ns-resize"));
        assert_eq!(at(200.0, 150.0), Some(Grip::MOVE));
        assert_eq!(at(20.0, 150.0), None);
    }

    #[test]
    fn dragging_moves_edges_within_bounds() {
        let l = layout();
        let left = Grip {
            left: true,
            top: false,
            right: false,
            bottom: false,
        };
        // 15 area pixels is a tenth of the 150-pixel-wide picture.
        let crop = l.dragged(Crop::FULL, left, 15.0, 40.0);
        assert!((crop.left - 0.1).abs() < 1e-6);
        assert_eq!((crop.top, crop.right, crop.bottom), (0.0, 1.0, 1.0));
        // An edge cannot cross the other.
        let crop = l.dragged(Crop::FULL, left, 1000.0, 0.0);
        assert!(crop.right - crop.left >= assistant_media::MIN_CROP - 1e-6);
        // Moving keeps the size and stays inside.
        let half = Crop {
            left: 0.25,
            top: 0.25,
            right: 0.75,
            bottom: 0.75,
        };
        let moved = l.dragged(half, Grip::MOVE, 1000.0, -1000.0);
        assert_eq!(
            moved,
            Crop {
                left: 0.5,
                top: 0.0,
                right: 1.0,
                bottom: 0.5
            }
        );
    }

    #[test]
    fn tab_steps_through_the_crop_then_leaves() {
        let mut order = vec![Grip::MOVE];
        while let Some(next) = order.last().unwrap().next(false) {
            order.push(next);
        }
        assert_eq!(order, Grip::CYCLE);
        assert_eq!(Grip::MOVE.next(true), None);
        assert_eq!(Grip::CYCLE[4].next(true), Some(Grip::CYCLE[3]));
        // A corner grabbed with the pointer steps on as the whole crop.
        let corner = Grip::edge(true, true, false, false);
        assert_eq!(corner.next(false), Some(Grip::CYCLE[1]));
    }

    #[test]
    fn a_nudge_moves_only_what_its_direction_can() {
        let l = layout();
        let right_edge = Grip::CYCLE[3];
        // Up and down leave a side edge where it is.
        assert_eq!(l.dragged(Crop::FULL, right_edge, 0.0, NUDGE), Crop::FULL);
        // One pixel of the 150-pixel-wide picture.
        let crop = l.dragged(Crop::FULL, right_edge, -NUDGE, 0.0);
        assert!((crop.right - (1.0 - 1.0 / 150.0)).abs() < 1e-6);
        assert_eq!((crop.left, crop.top, crop.bottom), (0.0, 0.0, 1.0));
    }
}
