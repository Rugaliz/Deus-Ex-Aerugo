//! `XTextLogWindow` (Extension 100474b0..10047dc0): lines of text, each with its colour and the time it has left. The
//! window's text is all lines joined by line breaks (`RegenerateText`; what its size follows); `Tick` counts the lines
//! down and drops the expired ones from the front unless the log is paused; `Draw` draws each line in its own colour,
//! from the bottom up when the text is taller than the window. The HUD's message log is one.
use crate::{Value, Vm};

impl Vm {
    /// `RegenerateText` (10047dc0): the lines joined by `\n`, then the parent is asked to reconfigure.
    fn text_log_regenerate(&mut self, w: u32) {
        let text = self.ui.logs.get(&w).map(|v| v.iter().map(|l| l.0.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        let _ = self.set_prop(w, "text", 0, Value::Str(text));
        self.ui_ask_parent_for_reconfigure(w);
    }

    /// `AddLog` (100476a0): the line expires `GetTickOffset() + textTimeout` seconds from now.
    pub(crate) fn text_log_add(&mut self, w: u32, text: String, color: [u8; 4]) {
        let left = self.wf_pub(w, "textTimeout");
        self.ui.logs.entry(w).or_default().push((text, color, left));
        self.text_log_regenerate(w);
    }

    /// `ClearLog` (10047810).
    pub(crate) fn text_log_clear(&mut self, w: u32) {
        self.ui.logs.remove(&w);
        self.text_log_regenerate(w);
    }

    /// `Tick` (10047c30): unless paused, every line's time goes down by `dt`; the expired lines at the front go.
    pub(crate) fn text_log_tick(&mut self, w: u32, dt: f32) {
        if self.wb_pub(w, "bPaused") {
            return;
        }
        let Some(lines) = self.ui.logs.get_mut(&w) else { return };
        for l in lines.iter_mut() {
            l.2 -= dt;
        }
        let n = lines.iter().take_while(|l| l.2 <= 0.0).count();
        if n > 0 {
            lines.drain(..n);
            self.text_log_regenerate(w);
        }
    }

    /// `ConfigurationChanged` (10047910): too tall when the text, wrapped to the window, does not fit.
    pub(crate) fn text_log_configured(&mut self, w: u32) {
        let g = self.wo_ref_pub(w, "gGc");
        if g == 0 {
            return;
        }
        let st = self.gc_state(g);
        let text = match self.get_prop(w, "text", 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        };
        let (hm, vm) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "vMargin"));
        let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
        let (_, h) = self.text_extent(&st, width - (hm + hm), &text);
        self.set_b_pub(w, "bTooTall", h > height - (vm + vm));
    }

    /// `Draw` (10047a10): word wrap on; each line in its colour at a running height (from the bottom, last line
    /// first, when too tall). The window's text is left as the last line drawn, as in the original.
    pub(crate) fn text_log_draw(&mut self, w: u32, g: u32) {
        let tall = self.wb_pub(w, "bTooTall");
        {
            let ha = self.wi_pub(w, "hAlign") as u8;
            let va = if tall { 2 } else { self.wi_pub(w, "vAlign") as u8 };
            let st = self.gc_mut(g);
            st.h_align = ha;
            st.v_align = va;
            st.word_wrap = true;
        }
        // no `DrawWindow` event: `XTextLogWindow::Draw` (10047a10) never runs the base classes' draw
        let lines = self.ui.logs.get(&w).cloned().unwrap_or_default();
        let (hm, vm) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "vMargin"));
        let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
        let mut y = vm;
        let order: Vec<usize> = if tall { (0..lines.len()).rev().collect() } else { (0..lines.len()).collect() };
        for i in order {
            let (text, color, _) = &lines[i];
            let _ = self.set_prop(w, "text", 0, Value::Str(text.clone()));
            let mut st = self.gc_state(g);
            let (_, h) = self.text_extent(&st, width - (hm + hm), text);
            self.gc_mut(g).text_color = *color;
            st.text_color = *color;
            self.gc_draw_text(&st, hm, y, width - (hm + hm), height - (vm + vm), text);
            if tall {
                y -= h;
            } else {
                y += h;
            }
        }
    }
}
