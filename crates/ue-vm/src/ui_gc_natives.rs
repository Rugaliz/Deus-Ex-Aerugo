//! The script-callable natives of the `GC` class, and `GetGC`/`ReleaseGC` of windows. See `ui_gc.rs`.

use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

fn fl(a: &[Value], i: usize, d: f32) -> f32 {
    match a.get(i) {
        None | Some(Value::Void) => d,
        Some(v) => v.as_f32().unwrap_or(d),
    }
}
fn bl(a: &[Value], i: usize, d: bool) -> bool {
    match a.get(i) {
        Some(Value::Void) | None => d,
        Some(v) => v.as_bool().unwrap_or(d),
    }
}
fn iv(a: &[Value], i: usize) -> i32 {
    a.get(i).and_then(Value::as_i32).unwrap_or(0)
}
fn ob(a: &[Value], i: usize) -> u32 {
    match a.get(i) {
        Some(Value::Obj(r)) => *r,
        _ => 0,
    }
}
fn col(a: &[Value], i: usize) -> [u8; 4] {
    match a.get(i) {
        Some(Value::Struct(_, b)) if b.len() >= 4 => [b[0], b[1], b[2], b[3]],
        _ => [0, 0, 0, 0],
    }
}

impl Vm {
    pub(crate) fn native_gc(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        use Value::*;
        // window natives that give out or give back a GC
        if name == "GetGC" || name == "ReleaseGC" {
            if !self.is_class(this, "Window") {
                return None;
            }
            let (w_gc, g_gc) = (self.wo_ref_pub(this, "wGc"), self.wo_ref_pub(this, "gGc"));
            if name == "GetGC" {
                if g_gc == 0 {
                    return Some(Ok(Obj(0)));
                }
                self.gc_push(g_gc);
                self.gc_copy_keep_stack(g_gc, w_gc);
                return Some(Ok(Obj(g_gc)));
            }
            let g = ob(a, 0);
            if g != 0 {
                self.gc_pop(g, None);
            }
            return Some(Ok(Void));
        }
        if !self.is_class(this, "GC") {
            return None;
        }
        let v = match name {
            "Intersect" => {
                self.gc_mut(this).intersect(fl(a, 0, 0.0), fl(a, 1, 0.0), fl(a, 2, 0.0), fl(a, 3, 0.0));
                Void
            }
            "EnableSmoothing" => {
                self.gc_mut(this).enable_smoothing(bl(a, 0, false));
                Void
            }
            "IsSmoothingEnabled" => Bool(self.gc_state(this).smoothed),
            "SetStyle" => {
                self.gc_mut(this).set_style(iv(a, 0) as u8);
                Void
            }
            "GetStyle" => Byte(self.gc_state(this).style),
            "EnableDrawing" => {
                self.gc_mut(this).draw_enabled = bl(a, 0, false);
                Void
            }
            "IsDrawingEnabled" => Bool(self.gc_state(this).draw_enabled),
            "EnableMasking" => {
                self.gc_mut(this).enable_masking(bl(a, 0, false));
                Void
            }
            "IsMaskingEnabled" => Bool(self.gc_state(this).masked),
            "EnableTranslucency" => {
                self.gc_mut(this).enable_translucency(bl(a, 0, false));
                Void
            }
            "IsTranslucencyEnabled" => Bool(self.gc_state(this).translucent),
            "EnableModulation" => {
                self.gc_mut(this).enable_modulation(bl(a, 0, false));
                Void
            }
            "IsModulationEnabled" => Bool(self.gc_state(this).modulated),
            "SetTileColor" => {
                self.gc_mut(this).tile_color = col(a, 0);
                Void
            }
            "SetTextColor" => {
                self.gc_mut(this).text_color = col(a, 0);
                Void
            }
            "GetTileColor" | "GetTextColor" => {
                let st = self.gc_state(this);
                let c = if name == "GetTileColor" { st.tile_color } else { st.text_color };
                a[0] = self.color_value_pub(c);
                Void
            }
            "SetFont" => {
                let f = ob(a, 0);
                let g = self.gc_mut(this);
                g.normal_font = f;
                g.bold_font = f;
                Void
            }
            "SetNormalFont" => {
                self.gc_mut(this).normal_font = ob(a, 0);
                Void
            }
            "SetBoldFont" => {
                self.gc_mut(this).bold_font = ob(a, 0);
                Void
            }
            "SetFonts" => {
                let (n, b) = (ob(a, 0), ob(a, 1));
                let g = self.gc_mut(this);
                g.normal_font = n;
                g.bold_font = b;
                Void
            }
            "GetFonts" => {
                let st = self.gc_state(this);
                a[0] = Obj(st.normal_font);
                a[1] = Obj(st.bold_font);
                Void
            }
            "EnableTranslucentText" => {
                self.gc_mut(this).enable_translucent_text(bl(a, 0, false));
                Void
            }
            "IsTranslucentTextEnabled" => Bool(self.gc_state(this).text_translucent),
            "SetTextVSpacing" => {
                self.gc_mut(this).v_spacing = fl(a, 0, 0.0);
                Void
            }
            "GetTextVSpacing" => Float(self.gc_state(this).v_spacing),
            "SetHorizontalAlignment" => {
                self.gc_mut(this).h_align = iv(a, 0) as u8;
                Void
            }
            "GetHorizontalAlignment" => Byte(self.gc_state(this).h_align),
            "SetVerticalAlignment" => {
                self.gc_mut(this).v_align = iv(a, 0) as u8;
                Void
            }
            "GetVerticalAlignment" => Byte(self.gc_state(this).v_align),
            "SetAlignments" => {
                let g = self.gc_mut(this);
                g.h_align = iv(a, 0) as u8;
                g.v_align = iv(a, 1) as u8;
                Void
            }
            "GetAlignments" => {
                let st = self.gc_state(this);
                a[0] = Byte(st.h_align);
                a[1] = Byte(st.v_align);
                Void
            }
            "EnableWordWrap" => {
                self.gc_mut(this).word_wrap = bl(a, 0, false);
                Void
            }
            "IsWordWrapEnabled" => Bool(self.gc_state(this).word_wrap),
            "EnableSpecialText" => {
                self.gc_mut(this).parse_meta = bl(a, 0, false);
                Void
            }
            "IsSpecialTextEnabled" => Bool(self.gc_state(this).parse_meta),
            "SetBaselineData" => {
                let g = self.gc_mut(this);
                g.baseline_offset = fl(a, 0, 2.0);
                g.underline_height = fl(a, 1, 1.0);
                Void
            }
            "CopyGC" => {
                let src = ob(a, 0);
                self.gc_copy(this, src);
                Void
            }
            "PushGC" => Int(self.gc_push(this)),
            "PopGC" => {
                // -1, passed or defaulted, pops one level (`execPopGC` calls `PopGC()`)
                let depth = a.first().and_then(|v| if matches!(v, Void) { None } else { v.as_i32() }).filter(|&d| d != -1);
                self.gc_pop(this, depth);
                Void
            }
            "GetTextExtent" => {
                let st = self.gc_state(this);
                let text = Self::gc_text_value(a.get(3).unwrap_or(&Void));
                let (w, h) = self.text_extent(&st, fl(a, 0, 0.0), &text);
                a[1] = Float(w);
                a[2] = Float(h);
                Void
            }
            "GetFontHeight" => {
                let st = self.gc_state(this);
                Float(self.font_height(&st, bl(a, 0, false)))
            }
            "DrawText" => {
                let st = self.gc_state(this);
                let text = Self::gc_text_value(a.get(4).unwrap_or(&Void));
                self.gc_draw_text(&st, fl(a, 0, 0.0), fl(a, 1, 0.0), fl(a, 2, 0.0), fl(a, 3, 0.0), &text);
                Void
            }
            "DrawIcon" => {
                let st = self.gc_state(this);
                let tex = ob(a, 2);
                if tex != 0 {
                    let (tw, th) = self.texture_size(tex);
                    self.gc_draw_stretched_icon(&st, fl(a, 0, 0.0), fl(a, 1, 0.0), tw, th, tex);
                }
                Void
            }
            "DrawTexture" => {
                let st = self.gc_state(this);
                let (w, h) = (fl(a, 2, 0.0), fl(a, 3, 0.0));
                self.gc_draw_icon_pattern(&st, fl(a, 0, 0.0), fl(a, 1, 0.0), w, h, fl(a, 4, 0.0), fl(a, 5, 0.0), w, h, ob(a, 6));
                Void
            }
            "DrawPattern" => {
                let st = self.gc_state(this);
                self.gc_draw_pattern(&st, fl(a, 0, 0.0), fl(a, 1, 0.0), fl(a, 2, 0.0), fl(a, 3, 0.0), fl(a, 4, 0.0), fl(a, 5, 0.0), ob(a, 6));
                Void
            }
            "DrawBox" => {
                let st = self.gc_state(this);
                self.gc_draw_box(&st, fl(a, 0, 0.0), fl(a, 1, 0.0), fl(a, 2, 0.0), fl(a, 3, 0.0), fl(a, 4, 0.0), fl(a, 5, 0.0), fl(a, 6, 0.0), ob(a, 7));
                Void
            }
            "DrawStretchedTexture" => {
                let st = self.gc_state(this);
                let tex = ob(a, 8);
                if tex != 0 && st.draw_enabled {
                    self.draw_tile(&st, tex, fl(a, 0, 0.0), fl(a, 1, 0.0), fl(a, 2, 0.0), fl(a, 3, 0.0), fl(a, 4, 0.0), fl(a, 5, 0.0), fl(a, 6, 0.0), fl(a, 7, 0.0), false, false);
                }
                Void
            }
            "DrawBorders" => {
                let st = self.gc_state(this);
                let mut tex = [0u32; 9];
                for (i, t) in tex.iter_mut().enumerate() {
                    *t = ob(a, 8 + i);
                }
                let (sh, sv) = (bl(a, 17, false), bl(a, 18, false));
                self.gc_draw_borders(&st, fl(a, 0, 0.0), fl(a, 1, 0.0), fl(a, 2, 0.0), fl(a, 3, 0.0), [fl(a, 4, 0.0), fl(a, 5, 0.0), fl(a, 6, 0.0), fl(a, 7, 0.0)], tex, sh, sv);
                Void
            }
            // `XGC::DrawActor`: for the length of the draw the actor is shown, lit or not as asked, in the GC's draw
            // style, with the given glow, its draw scale multiplied, and (if given) one skin everywhere; the draw goes to
            // the main view between the window quads, after a depth clear when asked. (`bConstrain` would fit the view
            // to the GC's clip area; no Deus Ex caller in the game uses it.)
            "DrawActor" => {
                let st = self.gc_state(this);
                let actor = ob(a, 0);
                if st.draw_enabled && self.ui.wants_views && actor != 0 && self.is_live(actor) {
                    let unlit = bl(a, 3, false);
                    let scale = fl(a, 4, 0.0);
                    let glow = fl(a, 5, 0.0);
                    let skin = ob(a, 6);
                    let saved_hidden = self.wb_pub(actor, "bHidden");
                    let saved_unlit = self.wb_pub(actor, "bUnlit");
                    let saved_style = self.u8p(actor, "Style");
                    let saved_glow = self.fp(actor, "ScaleGlow");
                    let saved_scale = self.fp(actor, "DrawScale");
                    let multi = self.prop_at(actor, "MultiSkins").map(|(off, _)| off);
                    let saved_skins: Vec<u32> = (0..8).map(|i| multi.map(|o| self.u32_at(actor, o + 4 * i)).unwrap_or(0)).collect();
                    let saved_skin = self.op(actor, "Skin");
                    if skin != 0 {
                        if let Some(o) = multi {
                            for i in 0..8 {
                                self.set_u32_at(actor, o + 4 * i, skin);
                            }
                        }
                        let _ = self.set_prop(actor, "Skin", 0, Obj(skin));
                    }
                    self.set_b_pub(actor, "bHidden", false);
                    self.set_b_pub(actor, "bUnlit", unlit);
                    self.set_u8p(actor, "Style", st.style);
                    self.set_fp(actor, "ScaleGlow", glow);
                    self.set_fp(actor, "DrawScale", scale * saved_scale);
                    let draw = self.mesh_draw(actor);
                    self.set_b_pub(actor, "bHidden", saved_hidden);
                    self.set_b_pub(actor, "bUnlit", saved_unlit);
                    self.set_u8p(actor, "Style", saved_style);
                    self.set_fp(actor, "ScaleGlow", saved_glow);
                    self.set_fp(actor, "DrawScale", saved_scale);
                    if skin != 0 {
                        if let Some(o) = multi {
                            for (i, r) in saved_skins.iter().enumerate() {
                                self.set_u32_at(actor, o + 4 * i, *r);
                            }
                        }
                        let _ = self.set_prop(actor, "Skin", 0, Obj(saved_skin));
                    }
                    self.ui.cmd_seq += 1;
                    let cmd = crate::ui_gc::ActorCmd { after: self.ui.draw.len(), seq: self.ui.cmd_seq, actor, draw, clear_z: bl(a, 1, false) };
                    self.ui.actor_cmds.push(cmd);
                }
                Void
            }
            "ClearZ" => {
                if self.ui.wants_views {
                    self.ui.cmd_seq += 1;
                    let cmd = crate::ui_gc::ActorCmd { after: self.ui.draw.len(), seq: self.ui.cmd_seq, actor: 0, draw: None, clear_z: true };
                    self.ui.actor_cmds.push(cmd);
                }
                Void
            }
            _ => return None,
        };
        Some(Ok(v))
    }

    /// `CopyGC` into a GC that has a state stack: only the drawing state changes.
    fn gc_copy_keep_stack(&mut self, dst: u32, src: u32) {
        self.gc_copy(dst, src);
    }
}
