//! Small, allocation-free Calculator application surface with keyboard, pointer,
//! and complete local accessibility semantics. Arithmetic uses Prism's bounded
//! integer parser; this panel never evaluates code or accepts external actions.

use crate::{
    accessibility::{self, Error as AccessibilityError, Observer, Role, Snapshot, State},
    calculator,
    theme::{self, Font},
    Bounds, Canvas, Key,
};
use core::fmt::Write;

pub const EXPRESSION_CAPACITY: usize = 96;
const CONTEXT: u32 = 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    Expression,
    Calculate,
    Clear,
}

impl Control {
    const fn index(self) -> usize {
        match self {
            Self::Expression => 0,
            Self::Calculate => 1,
            Self::Clear => 2,
        }
    }
    const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Calculate,
            2 => Self::Clear,
            _ => Self::Expression,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    Ignored,
    Edited,
    FocusChanged,
    Evaluated(Result<i64, calculator::Error>),
    Cleared,
}

#[derive(Clone, Copy)]
struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Text<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
    fn clear(&mut self) {
        self.bytes = [0; N];
        self.len = 0;
    }
    fn set(&mut self, text: &str) -> bool {
        if text.len() > N {
            return false;
        }
        self.clear();
        self.bytes[..text.len()].copy_from_slice(text.as_bytes());
        self.len = text.len();
        true
    }
}

impl<const N: usize> Write for Text<N> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        if self.len + text.len() > N {
            return Err(core::fmt::Error);
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}

pub struct CalculatorApp {
    expression: Text<EXPRESSION_CAPACITY>,
    result: Text<80>,
    value: Option<Result<i64, calculator::Error>>,
    focus: Control,
    observer: Observer,
}

impl Default for CalculatorApp {
    fn default() -> Self {
        Self::new()
    }
}

impl CalculatorApp {
    pub const fn new() -> Self {
        Self {
            expression: Text::new(),
            result: Text::new(),
            value: None,
            focus: Control::Expression,
            observer: Observer::new(),
        }
    }

    pub fn expression(&self) -> &str {
        self.expression.as_str()
    }
    pub fn result(&self) -> Option<Result<i64, calculator::Error>> {
        self.value
    }
    pub fn result_text(&self) -> &str {
        if self.result.len == 0 {
            "Enter an expression, then choose Calculate."
        } else {
            self.result.as_str()
        }
    }
    pub const fn focused(&self) -> Control {
        self.focus
    }

    /// Replaces the expression and evaluates it through the same bounded
    /// parser used by keyboard and pointer calculation.
    pub fn evaluate_expression(&mut self, expression: &str) -> Change {
        if !self.expression.set(expression) {
            return Change::Ignored;
        }
        self.focus = Control::Expression;
        self.calculate()
    }

    pub fn on_key(&mut self, key: Key) -> Change {
        match key {
            Key::Tab | Key::Down => {
                self.focus = Control::from_index((self.focus.index() + 1) % 3);
                Change::FocusChanged
            }
            Key::BackTab | Key::Up => {
                self.focus = Control::from_index((self.focus.index() + 2) % 3);
                Change::FocusChanged
            }
            Key::Escape => self.clear(),
            Key::Enter if self.focus == Control::Clear => self.clear(),
            Key::Enter => self.calculate(),
            Key::Delete if self.focus == Control::Expression => self.clear(),
            Key::Backspace if self.focus == Control::Expression => {
                if self.expression.len == 0 {
                    return Change::Ignored;
                }
                self.expression.len -= 1;
                self.expression.bytes[self.expression.len] = 0;
                self.invalidate_result();
                Change::Edited
            }
            Key::Character(character) if self.focus == Control::Expression => {
                if !Self::is_expression_character(character)
                    || self.expression.len == EXPRESSION_CAPACITY
                {
                    return Change::Ignored;
                }
                self.expression.bytes[self.expression.len] = character as u8;
                self.expression.len += 1;
                self.invalidate_result();
                Change::Edited
            }
            _ => Change::Ignored,
        }
    }

    pub fn on_pointer(&mut self, x: i32, y: i32, bounds: Bounds) -> Change {
        self.on_pointer_layout(x, y, bounds)
    }

    pub fn on_pointer_embedded(&mut self, x: i32, y: i32, bounds: Bounds) -> Change {
        self.on_pointer_layout(x, y, embedded_layout(bounds))
    }

    fn on_pointer_layout(&mut self, x: i32, y: i32, bounds: Bounds) -> Change {
        if contains(expression_bounds(bounds), x, y) {
            if self.focus == Control::Expression {
                return Change::Ignored;
            }
            self.focus = Control::Expression;
            return Change::FocusChanged;
        }
        if contains(calculate_bounds(bounds), x, y) {
            self.focus = Control::Calculate;
            return self.calculate();
        }
        if contains(clear_bounds(bounds), x, y) {
            self.focus = Control::Clear;
            return self.clear();
        }
        Change::Ignored
    }

    pub fn write_accessibility_snapshot(
        &mut self,
        out: &mut Snapshot,
        bounds: Bounds,
    ) -> Result<(), AccessibilityError> {
        self.write_snapshot(out, bounds, bounds)
    }

    pub fn write_accessibility_snapshot_embedded(
        &mut self,
        out: &mut Snapshot,
        bounds: Bounds,
    ) -> Result<(), AccessibilityError> {
        self.write_snapshot(out, embedded_layout(bounds), bounds)
    }

    fn write_snapshot(
        &mut self,
        out: &mut Snapshot,
        layout_bounds: Bounds,
        root_bounds: Bounds,
    ) -> Result<(), AccessibilityError> {
        let state = State::ENABLED;
        out.begin(CONTEXT, 0);
        out.add(
            1,
            None,
            Role::Window,
            "Next Calculator",
            "Basic integer arithmetic",
            Some(root_bounds),
            state,
        )?;
        out.add(
            2,
            None,
            Role::TextBox,
            "Expression",
            self.expression(),
            Some(expression_bounds(layout_bounds)),
            State {
                focused: self.focus == Control::Expression,
                ..state
            },
        )?;
        out.add(
            3,
            None,
            Role::Button,
            "Calculate",
            "Enter",
            Some(calculate_bounds(layout_bounds)),
            State {
                focused: self.focus == Control::Calculate,
                ..state
            },
        )?;
        out.add(
            4,
            None,
            Role::Button,
            "Clear",
            "Escape or Delete",
            Some(clear_bounds(layout_bounds)),
            State {
                focused: self.focus == Control::Clear,
                ..state
            },
        )?;
        out.add(
            5,
            None,
            Role::Status,
            "Calculation result",
            self.result_text(),
            Some(result_bounds(layout_bounds)),
            state,
        )?;
        self.observer.observe(out)
    }

    pub fn take_accessibility_event(&mut self) -> Option<accessibility::Event> {
        self.observer.take()
    }

    pub fn render(&self, canvas: &mut Canvas<'_>, bounds: Bounds) {
        self.render_layout(canvas, bounds, true)
    }

    pub fn render_embedded(&self, canvas: &mut Canvas<'_>, bounds: Bounds) {
        self.render_layout(canvas, embedded_layout(bounds), false)
    }

    fn render_layout(&self, canvas: &mut Canvas<'_>, bounds: Bounds, frame: bool) {
        let Some(bounds) = clip_bounds(canvas, bounds) else {
            return;
        };
        if frame {
            theme::shadow(canvas, bounds, 12);
            theme::rounded_rect(canvas, bounds, 12, theme::SURFACE);
            theme::outline(canvas, bounds, 12, theme::BORDER);
            theme::icon(
                canvas,
                theme::Icon::Next,
                bounds.x + 16,
                bounds.y + 12,
                20,
                theme::ACCENT,
            );
            theme::text(
                canvas,
                bounds.x + 44,
                bounds.y + 8,
                "Next Calculator",
                theme::TEXT,
                Font::Title,
            );
            canvas.rect(
                bounds.x + 1,
                bounds.y + 40,
                bounds.width - 2,
                1,
                theme::BORDER,
            );
        }
        theme::text(
            canvas,
            bounds.x + 18,
            bounds.y + 50,
            "Expression",
            theme::MUTED,
            Font::Small,
        );
        let input = expression_bounds(bounds);
        theme::rounded_rect(canvas, input, 8, theme::SURFACE_RAISED);
        let input_border = if self.focus == Control::Expression {
            theme::ACCENT
        } else {
            theme::BORDER
        };
        stroke(canvas, input, input_border);
        let input_text = if self.expression.len == 0 {
            "Type an integer expression"
        } else {
            self.expression()
        };
        let text_width = input.width.saturating_sub(24).max(0);
        let text = fit_suffix(input_text, text_width);
        theme::text(
            canvas,
            input.x + 12,
            input.y + 5,
            text,
            if self.expression.len == 0 {
                theme::MUTED
            } else {
                theme::TEXT
            },
            Font::Body,
        );
        if self.focus == Control::Expression {
            let caret_x = input.x + 12 + theme::text_width(text, Font::Body);
            canvas.rect(caret_x, input.y + 8, 2, 24, theme::ACCENT);
        }
        theme::text(
            canvas,
            bounds.x + 18,
            bounds.y + 130,
            "Result",
            theme::MUTED,
            Font::Small,
        );
        let result = result_bounds(bounds);
        theme::rounded_rect(canvas, result, 8, theme::ACCENT_SOFT);
        let value = fit_prefix(self.result_text(), result.width.saturating_sub(24).max(0));
        theme::text(
            canvas,
            result.x + 12,
            result.y + 4,
            value,
            if self.value.is_some_and(|value| value.is_err()) {
                theme::TEXT
            } else {
                theme::ACCENT
            },
            Font::Body,
        );
        for (control, button, label) in [
            (Control::Calculate, calculate_bounds(bounds), "Calculate"),
            (Control::Clear, clear_bounds(bounds), "Clear"),
        ] {
            let selected = control == self.focus;
            let fill = if control == Control::Calculate {
                theme::ACCENT
            } else {
                theme::SURFACE_RAISED
            };
            let foreground = if control == Control::Calculate {
                0xffffff
            } else {
                theme::TEXT
            };
            theme::rounded_rect(canvas, button, 8, fill);
            stroke(
                canvas,
                button,
                if selected { theme::TEXT } else { theme::BORDER },
            );
            let label_width = theme::text_width(label, Font::Body);
            theme::text(
                canvas,
                button.x + (button.width - label_width) / 2,
                button.y + 1,
                label,
                foreground,
                Font::Body,
            );
        }
        theme::text(
            canvas,
            bounds.x + 18,
            bounds.y + bounds.height - 30,
            "Tab changes focus   Enter calculates   Esc clears",
            theme::MUTED,
            Font::Small,
        );
    }

    fn calculate(&mut self) -> Change {
        let result = calculator::evaluate(self.expression());
        self.value = Some(result);
        self.result.clear();
        match result {
            Ok(value) => {
                let _ = write!(&mut self.result, "{value}");
            }
            Err(error) => {
                let _ = self.result.set(error.message());
            }
        }
        Change::Evaluated(result)
    }

    fn clear(&mut self) -> Change {
        self.expression.clear();
        self.invalidate_result();
        self.focus = Control::Expression;
        Change::Cleared
    }

    fn invalidate_result(&mut self) {
        self.value = None;
        self.result.clear();
    }

    const fn is_expression_character(character: char) -> bool {
        matches!(
            character,
            '0'..='9' | '+' | '-' | '*' | '/' | '%' | '(' | ')' | ' ' | '\t'
        )
    }
}

pub fn expression_bounds(bounds: Bounds) -> Bounds {
    Bounds {
        x: bounds.x.saturating_add(16),
        y: bounds.y.saturating_add(72),
        width: bounds.width.saturating_sub(32).max(0),
        height: 48,
    }
}
fn embedded_layout(bounds: Bounds) -> Bounds {
    Bounds {
        y: bounds.y.saturating_sub(40),
        height: bounds.height.saturating_add(40),
        ..bounds
    }
}
pub fn result_bounds(bounds: Bounds) -> Bounds {
    Bounds {
        x: bounds.x.saturating_add(16),
        y: bounds.y.saturating_add(156),
        width: bounds.width.saturating_sub(32).max(0),
        height: 40,
    }
}
pub fn calculate_bounds(bounds: Bounds) -> Bounds {
    let button_width = button_width(bounds);
    Bounds {
        x: bounds
            .x
            .saturating_add(bounds.width)
            .saturating_sub(button_width.saturating_mul(2))
            .saturating_sub(44),
        y: bounds.y.saturating_add(bounds.height).saturating_sub(52),
        width: button_width,
        height: 36,
    }
}
pub fn clear_bounds(bounds: Bounds) -> Bounds {
    let button_width = button_width(bounds);
    Bounds {
        x: bounds
            .x
            .saturating_add(bounds.width)
            .saturating_sub(button_width)
            .saturating_sub(16),
        y: bounds.y.saturating_add(bounds.height).saturating_sub(52),
        width: button_width,
        height: 36,
    }
}
fn button_width(bounds: Bounds) -> i32 {
    bounds.width.saturating_sub(48).clamp(0, 144) / 2
}
fn contains(bounds: Bounds, x: i32, y: i32) -> bool {
    bounds.width > 0
        && bounds.height > 0
        && x >= bounds.x
        && y >= bounds.y
        && x < bounds.x.saturating_add(bounds.width)
        && y < bounds.y.saturating_add(bounds.height)
}
fn stroke(canvas: &mut Canvas<'_>, bounds: Bounds, color: u32) {
    if bounds.width <= 0 || bounds.height <= 0 {
        return;
    }
    canvas.rect(bounds.x, bounds.y, bounds.width, 1, color);
    canvas.rect(
        bounds.x,
        bounds.y.saturating_add(bounds.height).saturating_sub(1),
        bounds.width,
        1,
        color,
    );
    canvas.rect(bounds.x, bounds.y, 1, bounds.height, color);
    canvas.rect(
        bounds.x.saturating_add(bounds.width).saturating_sub(1),
        bounds.y,
        1,
        bounds.height,
        color,
    );
}
fn clip_bounds(canvas: &Canvas<'_>, bounds: Bounds) -> Option<Bounds> {
    if bounds.width <= 0 || bounds.height <= 0 {
        return None;
    }
    let left = bounds.x.max(0);
    let top = bounds.y.max(0);
    let right = bounds
        .x
        .saturating_add(bounds.width)
        .min(canvas.width() as i32);
    let bottom = bounds
        .y
        .saturating_add(bounds.height)
        .min(canvas.height() as i32);
    (right > left && bottom > top).then_some(Bounds {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}
fn fit_prefix(text: &str, width: i32) -> &str {
    fit_text(text, width, false)
}
fn fit_suffix(text: &str, width: i32) -> &str {
    fit_text(text, width, true)
}
fn fit_text(text: &str, width: i32, suffix: bool) -> &str {
    if width <= 0 {
        return "";
    }
    if theme::text_width(text, Font::Body) <= width {
        return text;
    }
    if suffix {
        let mut start = text.len();
        for (index, _) in text.char_indices().rev() {
            if theme::text_width(&text[index..], Font::Body) > width {
                break;
            }
            start = index;
        }
        &text[start..]
    } else {
        let mut end = 0;
        for (index, character) in text.char_indices() {
            let next = index + character.len_utf8();
            if theme::text_width(&text[..next], Font::Body) > width {
                break;
            }
            end = next;
        }
        &text[..end]
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::PixelFormat;

    fn enter(app: &mut CalculatorApp, expression: &str) {
        for character in expression.chars() {
            app.on_key(Key::Character(character));
        }
    }

    #[test]
    fn keyboard_calculation_edits_without_exceeding_parser_contract() {
        let mut app = CalculatorApp::new();
        enter(&mut app, "(2 + 3) * 4");
        assert_eq!(app.expression(), "(2 + 3) * 4");
        assert_eq!(app.on_key(Key::Enter), Change::Evaluated(Ok(20)));
        assert_eq!(app.result_text(), "20");
        app.on_key(Key::Backspace);
        assert_eq!(app.expression(), "(2 + 3) * ");
        assert_eq!(app.result(), None);
        assert_eq!(
            app.on_key(Key::Enter),
            Change::Evaluated(Err(calculator::Error::Syntax))
        );
        assert_eq!(app.result_text(), calculator::Error::Syntax.message());
        assert_eq!(app.on_key(Key::Character('x')), Change::Ignored);
        app.on_key(Key::Tab);
        assert_eq!(app.focused(), Control::Calculate);
        app.on_key(Key::Escape);
        assert_eq!(app.expression(), "");
        assert_eq!(app.focused(), Control::Expression);
        assert_eq!(app.result(), None);
    }

    #[test]
    fn pointer_and_keyboard_controls_share_focus_and_clear_behavior() {
        let mut app = CalculatorApp::new();
        let bounds = Bounds {
            x: 40,
            y: 30,
            width: 560,
            height: 280,
        };
        enter(&mut app, "8 / 2");
        app.on_key(Key::Tab);
        assert_eq!(app.focused(), Control::Calculate);
        assert_eq!(app.on_key(Key::Enter), Change::Evaluated(Ok(4)));
        let clear = clear_bounds(bounds);
        assert_eq!(
            app.on_pointer(clear.x + 2, clear.y + 2, bounds),
            Change::Cleared
        );
        assert_eq!(app.expression(), "");
        assert_eq!(app.focused(), Control::Expression);
        let input = expression_bounds(bounds);
        app.on_key(Key::Tab);
        assert_eq!(
            app.on_pointer(input.x + 2, input.y + 2, bounds),
            Change::FocusChanged
        );
        assert_eq!(
            app.on_pointer(input.x + 2, input.y + 2, bounds),
            Change::Ignored
        );
        assert_eq!(app.on_pointer(bounds.x, bounds.y, bounds), Change::Ignored);
    }

    #[test]
    fn accessible_snapshot_exposes_actual_values_focus_and_live_change_events() {
        let mut app = CalculatorApp::new();
        let bounds = Bounds {
            x: 40,
            y: 30,
            width: 560,
            height: 280,
        };
        let mut snapshot = Snapshot::empty();
        app.write_accessibility_snapshot(&mut snapshot, bounds)
            .unwrap();
        assert!(snapshot.complete);
        assert_eq!(snapshot.context, CONTEXT);
        assert_eq!(snapshot.nodes()[0].name(), "Next Calculator");
        assert_eq!(snapshot.nodes()[0].role, Role::Window);
        assert_eq!(snapshot.nodes()[0].bounds, Some(bounds));
        let expression = snapshot
            .nodes()
            .iter()
            .find(|node| node.name() == "Expression")
            .unwrap();
        assert_eq!(expression.role, Role::TextBox);
        assert_eq!(expression.bounds, Some(expression_bounds(bounds)));
        assert!(expression.state.focused);
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|node| node.name() == "Calculate")
                .unwrap()
                .bounds,
            Some(calculate_bounds(bounds))
        );
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|node| node.name() == "Clear")
                .unwrap()
                .bounds,
            Some(clear_bounds(bounds))
        );
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|node| node.name() == "Calculation result")
                .unwrap()
                .bounds,
            Some(result_bounds(bounds))
        );
        enter(&mut app, "9 * 9");
        app.on_key(Key::Tab);
        app.write_accessibility_snapshot(&mut snapshot, bounds)
            .unwrap();
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|node| node.state.focused)
                .unwrap()
                .name(),
            "Calculate"
        );
        assert!(core::iter::from_fn(|| app.take_accessibility_event())
            .any(|event| event.kind == accessibility::EventKind::FocusChanged));
        app.on_key(Key::Enter);
        app.write_accessibility_snapshot(&mut snapshot, bounds)
            .unwrap();
        assert_eq!(
            snapshot
                .nodes()
                .iter()
                .find(|node| node.name() == "Calculation result")
                .unwrap()
                .value(),
            "81"
        );
    }

    #[test]
    fn evaluate_expression_prefills_and_uses_the_regular_result_state() {
        let mut app = CalculatorApp::new();
        assert_eq!(
            app.evaluate_expression("(2 + 3) * 4"),
            Change::Evaluated(Ok(20))
        );
        assert_eq!(app.expression(), "(2 + 3) * 4");
        assert_eq!(app.result(), Some(Ok(20)));
        assert_eq!(app.result_text(), "20");
        assert_eq!(app.focused(), Control::Expression);

        assert_eq!(
            app.evaluate_expression("1 / 0"),
            Change::Evaluated(Err(calculator::Error::DivisionByZero))
        );
        assert_eq!(app.expression(), "1 / 0");
        assert_eq!(app.result(), Some(Err(calculator::Error::DivisionByZero)));
    }

    #[test]
    fn renderer_clips_extreme_bounds_and_preserves_guard_pixels() {
        let app = CalculatorApp::new();
        let width = 640;
        let height = 360;
        let mut pixels = std::vec![0xdeadbeef; width * height + 2];
        let end = pixels.len() - 1;
        let mut canvas =
            Canvas::new(&mut pixels[1..end], width, height, width, PixelFormat::Bgr).unwrap();
        app.render(
            &mut canvas,
            Bounds {
                x: -100,
                y: -100,
                width: 900,
                height: 700,
            },
        );
        app.render(
            &mut canvas,
            Bounds {
                x: i32::MAX,
                y: i32::MAX,
                width: i32::MAX,
                height: i32::MAX,
            },
        );
        assert_eq!(pixels[0], 0xdeadbeef);
        assert_eq!(pixels[end], 0xdeadbeef);
        assert!(pixels[1..end].iter().any(|pixel| *pixel != 0xdeadbeef));
    }
}
