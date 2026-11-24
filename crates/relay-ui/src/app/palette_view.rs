//! Command palette rendering, shaped text measurement, and GPUI IME input handler.

use super::*;

/// The last shaped palette line and its window bounds. The input handler uses
/// it to place the IME candidate window and to map clicks to character
/// indices without re-shaping outside paint.
#[derive(Clone)]
pub(super) struct PaletteTextMetrics {
    pub(super) line: ShapedLine,
    pub(super) bounds: Bounds<Pixels>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) cursor: Pixels,
}

struct PaletteTextPrepaint {
    line: ShapedLine,
    cursor: Pixels,
    selection: Option<(Pixels, Pixels)>,
}

/// Split the query into text runs so the IME-marked range can be underlined
/// without changing the rest of the line. Lengths are UTF-8 bytes, as
/// `TextRun` expects.
pub(super) fn palette_text_runs(query: &str, marked: Option<Range<usize>>, base: &TextRun, accent: Hsla) -> Vec<TextRun> {
    let Some(marked) = marked.filter(|range| {
        range.start < range.end
            && range.end <= query.len()
            && query.is_char_boundary(range.start)
            && query.is_char_boundary(range.end)
    }) else {
        return vec![base.clone()];
    };
    let mut runs = Vec::with_capacity(3);
    if marked.start > 0 {
        runs.push(TextRun { len: marked.start, ..base.clone() });
    }
    runs.push(TextRun {
        len: marked.end - marked.start,
        underline: Some(UnderlineStyle { thickness: px(1.), color: Some(accent), wavy: false }),
        ..base.clone()
    });
    if marked.end < query.len() {
        runs.push(TextRun { len: query.len() - marked.end, ..base.clone() });
    }
    runs.retain(|run| run.len > 0);
    runs
}

/// The palette query as a real text element instead of one div per character.
/// It shapes the line, renders the caret/selection/marked underline, installs
/// the platform `InputHandler` during paint, and maps clicks back to the
/// palette's byte cursor.
struct PaletteTextElement {
    app: Entity<RelayApp>,
    focus: FocusHandle,
    theme: Theme,
    metrics: Rc<RefCell<Option<PaletteTextMetrics>>>,
    query: SharedString,
    cursor: usize,
    selection: Option<(usize, usize)>,
    marked: Option<Range<usize>>,
}

impl IntoElement for PaletteTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for PaletteTextElement {
    type RequestLayoutState = ();
    type PrepaintState = PaletteTextPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("palette-text".into()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let placeholder = self.query.is_empty();
        let text: SharedString = if placeholder { "Search or enter address…".into() } else { self.query.clone() };

        let mut style = window.text_style();
        style.font_family = ".SystemUIFont".into();
        style.font_size = px(15.).into();
        style.color = if placeholder { self.theme.muted } else { self.theme.text };
        let base = TextRun {
            len: text.len(),
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = palette_text_runs(&self.query, self.marked.clone(), &base, self.theme.accent);
        let line = window.text_system().shape_line(text, style.font_size.to_pixels(window.rem_size()), &runs, None);
        let cursor = if placeholder { px(0.) } else { line.x_for_index(self.cursor.min(self.query.len())) };
        let selection = if placeholder {
            None
        } else {
            self.selection.map(|(start, end)| (line.x_for_index(start), line.x_for_index(end)))
        };
        *self.metrics.borrow_mut() = Some(PaletteTextMetrics { line: line.clone(), bounds, cursor });
        PaletteTextPrepaint { line, cursor, selection }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focused = self.focus.is_focused(window);
        if focused {
            window.handle_input(&self.focus, ElementInputHandler::new(bounds, self.app.clone()), cx);
        }
        self.app.update(cx, |app, _| app.palette_input_active = focused);

        if let Some((start, end)) = prepaint.selection.take() {
            window.paint_quad(fill(
                Bounds::new(point(bounds.left() + start, bounds.top()), size((end - start).max(px(1.)), bounds.size.height)),
                self.theme.active,
            ));
        }
        prepaint.line.paint(bounds.origin, bounds.size.height, window, cx).ok();
        if focused {
            window.paint_quad(fill(
                Bounds::new(point(bounds.left() + prepaint.cursor, bounds.top()), size(px(1.5), px(18.))),
                self.theme.accent,
            ));
        }

        let app = self.app.clone();
        let metrics = self.metrics.clone();
        let hit_bounds = bounds;
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !hit_bounds.contains(&event.position) {
                return;
            }
            let byte = {
                let metrics = metrics.borrow();
                let Some(metrics) = metrics.as_ref() else { return };
                metrics
                    .line
                    .index_for_x(event.position.x - metrics.bounds.left())
                    .unwrap_or_else(|| metrics.line.len())
            };
            app.update(cx, |app, cx| {
                if let Some(palette) = app.palette.as_mut() {
                    palette.set_cursor(byte, event.modifiers.shift);
                }
                cx.notify();
            });
        });
    }
}

impl RelayApp {
    pub(super) fn render_palette(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut list = div().flex().flex_col().py_1();
        for (i, item) in p.items.iter().enumerate() {
            let on = i == p.selected;
            let custom_key = match item {
                Item::Command(c) => c.action().map(|a| keymap::shortcut_label(self.keymap.key(a))).unwrap_or_default(),
                _ => item.detail(),
            };
            let item_c = item.clone();
            list = list.child(
                div()
                    .id(("pal", i))
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(32.))
                    .px_3()
                    .mx_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.active))
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.close_palette();
                        app.run_item(item_c.clone(), cx);
                        cx.notify();
                    }))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(13.)).child(SharedString::from(item.title())))
                    .child(div().text_size(px(12.)).text_color(t.muted).child(SharedString::from(custom_key))),
            );
        }
        let query_row = div().flex_1().h_full().overflow_hidden().child(PaletteTextElement {
            app: cx.entity(),
            focus: self.focus.clone(),
            theme: t,
            metrics: self.palette_metrics.clone(),
            query: p.query.clone().into(),
            cursor: p.cursor,
            selection: p.selection(),
            marked: p.marked.clone(),
        });
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(90.)).bg(t.scrim).child(
            div()
                .id("palette")
                .w(px(560.))
                .max_h(px(480.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                    app.close_palette();
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(t.border)
                        .child(query_row),
                )
                .child(list),
        )
    }

}

impl EntityInputHandler for RelayApp {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let palette = self.palette.as_ref()?;
        let start = palette::utf16_to_byte(&palette.query, range_utf16.start);
        let end = palette::utf16_to_byte(&palette.query, range_utf16.end);
        let start = start.min(end);
        let end = end.max(start);
        *adjusted_range = Some(palette::byte_to_utf16(&palette.query, start)..palette::byte_to_utf16(&palette.query, end));
        Some(palette.query[start..end].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let palette = self.palette.as_ref()?;
        let (start, end) = palette.selection().unwrap_or((palette.cursor, palette.cursor));
        Some(UTF16Selection {
            range: palette::byte_to_utf16(&palette.query, start)..palette::byte_to_utf16(&palette.query, end),
            reversed: palette.anchor.is_some_and(|anchor| anchor > palette.cursor),
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        let palette = self.palette.as_ref()?;
        palette
            .marked
            .as_ref()
            .map(|range| palette::byte_to_utf16(&palette.query, range.start)..palette::byte_to_utf16(&palette.query, range.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(palette) = self.palette.as_mut() {
            palette.unmark();
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = self.palette.as_mut() else { return };
        let range = palette.replacement_range(range_utf16);
        palette.replace_range(range, text);
        palette.refresh(&self.sources, &self.search);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = self.palette.as_mut() else { return };
        let range = palette.replacement_range(range_utf16);
        palette.replace_and_mark(range, new_text, new_selected_range);
        palette.refresh(&self.sources, &self.search);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let palette = self.palette.as_ref()?;
        let metrics = self.palette_metrics.borrow();
        let metrics = metrics.as_ref()?;
        let start = palette::utf16_to_byte(&palette.query, range_utf16.start).min(palette.query.len());
        let end = palette::utf16_to_byte(&palette.query, range_utf16.end).min(palette.query.len());
        let start_x = metrics.line.x_for_index(start.min(end));
        let end_x = metrics.line.x_for_index(end.max(start));
        Some(Bounds::new(
            point(metrics.bounds.left() + start_x, metrics.bounds.top()),
            size((end_x - start_x).max(px(1.)), metrics.bounds.size.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let palette = self.palette.as_ref()?;
        let metrics = self.palette_metrics.borrow();
        let metrics = metrics.as_ref()?;
        let x = point.x - metrics.bounds.left();
        let byte = if x <= px(0.) {
            0
        } else {
            metrics.line.index_for_x(x).unwrap_or_else(|| metrics.line.len())
        };
        Some(palette::byte_to_utf16(&palette.query, byte.min(palette.query.len())))
    }
}
