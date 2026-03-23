//! Border painting, box-shadow, and triangle/quad rasterization.

use crate::css::{ComputedStyle, ComputedValue};
use crate::layout::{LayoutBox, Rect};

use super::color::{parse_color, Color};
use super::{
    border_box_rect, border_color, border_radius_corners, Canvas,
    color_property, fill_triangle_clipped, fill_triangle_clipped_inclusive,
    has_border_radius, length_property, normalize_rect, padding_box_rect,
    resolve_color_value,
    BorderRegion,
};

pub(crate) fn paint_borders(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    style: &ComputedStyle,
    clip: Option<Rect>,
) {
    if !has_any_solid_border(style) {
        return;
    }
    let border_box = border_box_rect(layout);
    let padding_box = padding_box_rect(layout);
    let border = layout.dimensions.border;

    if has_border_radius(style) {
        // 角丸ありのボーダー: ボーダー領域（border_box の角丸内 かつ padding_box の角丸外）を
        // 各サイドの色で塗る。
        let (tl, tr, br, bl) = border_radius_corners(style);
        // 内側コーナー半径（paddingbox側）
        let inner_tl = (tl - border.left.min(border.top)).max(0.0);
        let inner_tr = (tr - border.right.min(border.top)).max(0.0);
        let inner_br = (br - border.right.min(border.bottom)).max(0.0);
        let inner_bl = (bl - border.left.min(border.bottom)).max(0.0);

        // 描画順: Left/Right を先（フル高さで描画）、その後 Top/Bottom でコーナーを上書きして色が勝つ
        // left border
        if border.left > 0.0 && has_solid_border_side(style, "left") {
            let color = border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0));
            canvas.fill_rounded_rect_annulus(
                border_box,
                tl, tr, br, bl,
                padding_box,
                inner_tl, inner_tr, inner_br, inner_bl,
                color,
                clip,
                BorderRegion::Left,
                border.left,
            );
        }
        // right border
        if border.right > 0.0 && has_solid_border_side(style, "right") {
            let color = border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0));
            canvas.fill_rounded_rect_annulus(
                border_box,
                tl, tr, br, bl,
                padding_box,
                inner_tl, inner_tr, inner_br, inner_bl,
                color,
                clip,
                BorderRegion::Right,
                border.right,
            );
        }
        // top border（コーナー優先）
        if border.top > 0.0 && has_solid_border_side(style, "top") {
            let color = border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0));
            canvas.fill_rounded_rect_annulus(
                border_box,
                tl, tr, br, bl,
                padding_box,
                inner_tl, inner_tr, inner_br, inner_bl,
                color,
                clip,
                BorderRegion::Top,
                border.top,
            );
        }
        // bottom border（コーナー優先）
        if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
            let color = border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0));
            canvas.fill_rounded_rect_annulus(
                border_box,
                tl, tr, br, bl,
                padding_box,
                inner_tl, inner_tr, inner_br, inner_bl,
                color,
                clip,
                BorderRegion::Bottom,
                border.bottom,
            );
        }
        return;
    }

    if border.top > 0.0 && has_solid_border_side(style, "top") {
        canvas.fill_rect_clipped(
            Rect {
                x: border_box.x,
                y: border_box.y,
                width: border_box.width,
                height: border.top,
            },
            border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
        canvas.fill_rect_clipped(
            Rect {
                x: border_box.x,
                y: padding_box.y + padding_box.height,
                width: border_box.width,
                height: border.bottom,
            },
            border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.left > 0.0 && has_solid_border_side(style, "left") {
        canvas.fill_rect_clipped(
            Rect {
                x: border_box.x,
                y: border_box.y + border.top,
                width: border.left,
                height: border_box.height - border.top - border.bottom,
            },
            border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.right > 0.0 && has_solid_border_side(style, "right") {
        canvas.fill_rect_clipped(
            Rect {
                x: padding_box.x + padding_box.width,
                y: border_box.y + border.top,
                width: border.right,
                height: border_box.height - border.top - border.bottom,
            },
            border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
}

pub(crate) fn has_solid_border_side(style: &ComputedStyle, side: &str) -> bool {
    if matches!(
        style.get(&format!("border-{side}-style")),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("solid")
    ) {
        return true;
    }

    matches!(
        style.get("border-style"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("solid")
    )
}

pub(crate) fn border_color_side(style: &ComputedStyle, side: &str) -> Option<Color> {
    resolve_color_value(style.get(&format!("border-{side}-color")), style)
        .or_else(|| border_color(style))
}

#[derive(Clone, Copy)]
pub(crate) struct EdgeSizesForPaint {
    pub(crate) top: f32,
    pub(crate) right: f32,
    pub(crate) bottom: f32,
    pub(crate) left: f32,
}

impl EdgeSizesForPaint {
    pub(crate) fn from_style(style: &ComputedStyle) -> Self {
        Self {
            top: length_property(style, "border-top-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
            right: length_property(style, "border-right-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
            bottom: length_property(style, "border-bottom-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
            left: length_property(style, "border-left-width")
                .or_else(|| length_property(style, "border-width"))
                .unwrap_or(0.0),
        }
    }

    pub(crate) fn total_horizontal(self) -> f32 {
        self.left + self.right
    }

    pub(crate) fn total_vertical(self) -> f32 {
        self.top + self.bottom
    }
}

pub(crate) fn paint_rect_borders(
    canvas: &mut Canvas,
    rect: Rect,
    style: &ComputedStyle,
    border: EdgeSizesForPaint,
    clip: Option<Rect>,
) {
    if border.top > 0.0 && has_solid_border_side(style, "top") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: border.top,
            },
            border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x,
                y: rect.y + rect.height - border.bottom,
                width: rect.width,
                height: border.bottom,
            },
            border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.left > 0.0 && has_solid_border_side(style, "left") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x,
                y: rect.y + border.top,
                width: border.left,
                height: (rect.height - border.top - border.bottom).max(0.0),
            },
            border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
    if border.right > 0.0 && has_solid_border_side(style, "right") {
        canvas.fill_rect_clipped(
            Rect {
                x: rect.x + rect.width - border.right,
                y: rect.y + border.top,
                width: border.right,
                height: (rect.height - border.top - border.bottom).max(0.0),
            },
            border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0)),
            clip,
        );
    }
}

pub(crate) fn paint_zero_sized_border_box(
    canvas: &mut Canvas,
    rect: Rect,
    style: &ComputedStyle,
    border: EdgeSizesForPaint,
    clip: Option<Rect>,
) {
    let inner_left = rect.x + border.left;
    let inner_top = rect.y + border.top;
    let inner_right = rect.x + rect.width - border.right;
    let inner_bottom = rect.y + rect.height - border.bottom;

    let paint_top = |canvas: &mut Canvas| {
        if border.top > 0.0 && has_solid_border_side(style, "top") {
            fill_quad_clipped(
                canvas,
                (rect.x, rect.y),
                (rect.x + rect.width, rect.y),
                (inner_right, inner_top),
                (inner_left, inner_top),
                border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };
    let paint_bottom = |canvas: &mut Canvas| {
        if border.bottom > 0.0 && has_solid_border_side(style, "bottom") {
            fill_quad_clipped(
                canvas,
                (rect.x, rect.y + rect.height),
                (rect.x + rect.width, rect.y + rect.height),
                (inner_right, inner_bottom),
                (inner_left, inner_bottom),
                border_color_side(style, "bottom").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };
    let paint_left = |canvas: &mut Canvas| {
        if border.left > 0.0 && has_solid_border_side(style, "left") {
            fill_quad_clipped(
                canvas,
                (rect.x, rect.y),
                (rect.x, rect.y + rect.height),
                (inner_left, inner_bottom),
                (inner_left, inner_top),
                border_color_side(style, "left").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };
    let paint_right = |canvas: &mut Canvas| {
        if border.right > 0.0 && has_solid_border_side(style, "right") {
            fill_quad_clipped(
                canvas,
                (rect.x + rect.width, rect.y),
                (rect.x + rect.width, rect.y + rect.height),
                (inner_right, inner_bottom),
                (inner_right, inner_top),
                border_color_side(style, "right").unwrap_or(Color::rgb(0, 0, 0)),
                clip,
            );
        }
    };

    if border.top == 0.0 && border.bottom > 0.0 {
        paint_bottom(canvas);
        paint_left(canvas);
        paint_right(canvas);
    } else if border.bottom == 0.0 && border.top > 0.0 {
        paint_left(canvas);
        paint_right(canvas);
        if has_solid_border_side(style, "top") {
            let color = border_color_side(style, "top").unwrap_or(Color::rgb(0, 0, 0));
            fill_triangle_clipped_inclusive(
                canvas,
                (rect.x, rect.y),
                (rect.x + rect.width, rect.y),
                (inner_right, inner_top),
                color,
                clip,
            );
            fill_triangle_clipped_inclusive(
                canvas,
                (rect.x, rect.y),
                (inner_right, inner_top),
                (inner_left, inner_top),
                color,
                clip,
            );
        }
    } else {
        paint_top(canvas);
        paint_bottom(canvas);
        paint_left(canvas);
        paint_right(canvas);
    }
}

pub(crate) fn fill_quad_clipped(
    canvas: &mut Canvas,
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    p4: (f32, f32),
    color: Color,
    clip: Option<Rect>,
) {
    fill_triangle_clipped(canvas, p1, p2, p3, color, clip);
    fill_triangle_clipped(canvas, p1, p3, p4, color, clip);
}

pub(crate) fn has_any_solid_border(style: &ComputedStyle) -> bool {
    ["top", "right", "bottom", "left"]
        .into_iter()
        .any(|side| has_solid_border_side(style, side))
}

/// `box-shadow` 宣言から解析した影のパラメータ。
#[derive(Debug, Clone)]
pub(crate) struct BoxShadow {
    pub(crate) offset_x: f32,
    pub(crate) offset_y: f32,
    pub(crate) blur_radius: f32,
    pub(crate) spread_radius: f32,
    pub(crate) color: Color,
    pub(crate) inset: bool,
}

/// `box-shadow` プロパティ値の文字列を解析して `BoxShadow` のリストを返す。
///
/// 書式: `[inset] <offset-x> <offset-y> [<blur>] [<spread>] [<color>]`
/// 複数の影はカンマ区切りで指定できる（rgba()/rgb() など関数の引数内のカンマは区切りとして扱わない）。
pub(crate) fn parse_box_shadow(value: &str) -> Vec<BoxShadow> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("none") || trimmed.is_empty() {
        return Vec::new();
    }

    // カンマ区切りで複数の影に分割（rgba() 内のカンマは無視）
    let shadow_strs = split_box_shadow_layers(trimmed);

    let mut shadows = Vec::new();
    for shadow_str in &shadow_strs {
        if let Some(shadow) = parse_single_box_shadow(shadow_str.trim()) {
            shadows.push(shadow);
        }
    }
    shadows
}

/// box-shadow 値をカンマで分割する（関数内のカンマは無視）。
pub(crate) fn split_box_shadow_layers(value: &str) -> Vec<String> {
    let mut layers = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                layers.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        layers.push(current.trim().to_string());
    }
    layers
}

/// 単一の box-shadow トークン列を解析する。
fn parse_single_box_shadow(value: &str) -> Option<BoxShadow> {
    // トークン化: 数値（px付き）、色文字列、キーワードに分割
    let tokens = tokenize_shadow_value(value);

    let mut inset = false;
    let mut lengths: Vec<f32> = Vec::new();
    let mut color: Option<Color> = None;

    for token in &tokens {
        let lower = token.to_ascii_lowercase();
        if lower == "inset" {
            inset = true;
        } else if let Some(px) = parse_shadow_length(token) {
            lengths.push(px);
        } else if let Some(c) = parse_color(token) {
            color = Some(c);
        }
    }

    // offset-x, offset-y は必須
    if lengths.len() < 2 {
        return None;
    }

    let offset_x = lengths[0];
    let offset_y = lengths[1];
    let blur_radius = lengths.get(2).copied().unwrap_or(0.0).max(0.0);
    let spread_radius = lengths.get(3).copied().unwrap_or(0.0);
    let color = color.unwrap_or(Color::rgba(0, 0, 0, 255));

    Some(BoxShadow {
        offset_x,
        offset_y,
        blur_radius,
        spread_radius,
        color,
        inset,
    })
}

/// shadow 値文字列を空白で分割（関数呼び出しはひとまとめに）。
fn tokenize_shadow_value(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
                if depth == 0 && !current.is_empty() {
                    tokens.push(current.trim().to_string());
                    current.clear();
                }
            }
            ' ' | '\t' if depth == 0 => {
                let tok = current.trim().to_string();
                if !tok.is_empty() {
                    tokens.push(tok);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let tok = current.trim().to_string();
    if !tok.is_empty() {
        tokens.push(tok);
    }
    tokens
}

/// CSS length 文字列（例: "10px", "-5px", "0"）を px 値として解析する。
fn parse_shadow_length(s: &str) -> Option<f32> {
    let s = s.trim();
    if s.ends_with("px") {
        s[..s.len() - 2].parse::<f32>().ok()
    } else if s == "0" {
        Some(0.0)
    } else {
        // 単純な数値（単位なし）も許容
        let v: Result<f32, _> = s.parse();
        v.ok()
    }
}

/// box-shadow をキャンバスに描画する（背景・ボーダーの前に描画）。
pub(crate) fn paint_box_shadow(
    canvas: &mut Canvas,
    style: &ComputedStyle,
    border_box: Rect,
    clip: Option<Rect>,
) {
    let shadow_value = match style.get("box-shadow") {
        Some(ComputedValue::Keyword(v)) => v.clone(),
        _ => return,
    };

    let shadows = parse_box_shadow(&shadow_value);
    let radii = if has_border_radius(style) {
        let (tl, tr, br, bl) = border_radius_corners(style);
        Some((tl, tr, br, bl))
    } else {
        None
    };

    // CSS 仕様では後ろに書いた影ほど下に描画される（逆順で描画）
    for shadow in shadows.iter().rev() {
        if shadow.inset {
            // inset shadow は未実装（スキップ）
            continue;
        }
        paint_outer_box_shadow(canvas, border_box, shadow, clip, radii);
    }
}

/// アウター box-shadow を描画する。
/// border_box の内側は描画しない（外側のみ）。
pub(crate) fn paint_outer_box_shadow(
    canvas: &mut Canvas,
    border_box: Rect,
    shadow: &BoxShadow,
    clip: Option<Rect>,
    radii: Option<(f32, f32, f32, f32)>,
) {
    let spread = shadow.spread_radius;
    let blur = shadow.blur_radius;

    // shadow の矩形（spread 分だけ拡張）
    let shadow_rect = Rect {
        x: border_box.x + shadow.offset_x - spread,
        y: border_box.y + shadow.offset_y - spread,
        width: border_box.width + spread * 2.0,
        height: border_box.height + spread * 2.0,
    };

    // 負の spread により shadow が縮んで消滅した場合は描画をスキップする
    if shadow_rect.width <= 0.0 || shadow_rect.height <= 0.0 {
        return;
    }

    let color = shadow.color;

    if blur <= 0.0 {
        if let Some((tl, tr, br, bl)) = radii {
            // blur=0 かつ border-radius あり: 一時バッファに rounded rect を描画し、
            // border_box 内側のアルファを消去してから合成する（矩形バンドでは角丸が反映されない）。
            let buf_x = shadow_rect.x.floor() as i32;
            let buf_y = shadow_rect.y.floor() as i32;
            let buf_w = shadow_rect.width.ceil() as u32 + 2;
            let buf_h = shadow_rect.height.ceil() as u32 + 2;
            if buf_w == 0 || buf_h == 0 {
                return;
            }
            let mut shadow_buf = Canvas::new(buf_w, buf_h);
            let local_rect = Rect {
                x: shadow_rect.x - buf_x as f32,
                y: shadow_rect.y - buf_y as f32,
                width: shadow_rect.width,
                height: shadow_rect.height,
            };
            shadow_buf.fill_rounded_rect(
                local_rect,
                Color::rgba(color.r, color.g, color.b, 255),
                tl + spread,
                tr + spread,
                br + spread,
                bl + spread,
                None,
            );
            // border_box 内側のアルファをゼロにして要素本体を除外する
            let box_local = Rect {
                x: border_box.x - buf_x as f32,
                y: border_box.y - buf_y as f32,
                width: border_box.width,
                height: border_box.height,
            };
            if let Some(box_local_n) = normalize_rect(box_local) {
                let x0 = box_local_n.x.floor().max(0.0) as i32;
                let y0 = box_local_n.y.floor().max(0.0) as i32;
                let x1 = (box_local_n.x + box_local_n.width).ceil().min(buf_w as f32) as i32;
                let y1 = (box_local_n.y + box_local_n.height).ceil().min(buf_h as f32) as i32;
                for py in y0..y1 {
                    for px in x0..x1 {
                        let idx = (py as u32 * buf_w + px as u32) as usize * 4;
                        shadow_buf.pixels[idx + 3] = 0;
                    }
                }
            }
            // color.a を alpha スケールとして適用
            let alpha_scale = color.a as f32 / 255.0;
            if alpha_scale < 1.0 {
                shadow_buf.multiply_alpha(alpha_scale);
            }
            canvas.composite_canvas_clipped(&shadow_buf, buf_x, buf_y, color.r, color.g, color.b, clip);
        } else {
            // blur なし・角丸なし: shadow_rect のうち border_box の外側のみを矩形バンドで描画する。
            let sx = shadow_rect.x;
            let sy = shadow_rect.y;
            let sw = shadow_rect.width;
            let sh = shadow_rect.height;
            let bx = border_box.x;
            let by = border_box.y;
            let bw = border_box.width;
            let bh = border_box.height;

            // top band: shadow の上端から border_box の上端まで
            if sy < by {
                canvas.fill_rect_clipped(
                    Rect { x: sx, y: sy, width: sw, height: by - sy },
                    color,
                    clip,
                );
            }
            // bottom band: border_box の下端から shadow の下端まで
            let shadow_bottom = sy + sh;
            let box_bottom = by + bh;
            if shadow_bottom > box_bottom {
                canvas.fill_rect_clipped(
                    Rect { x: sx, y: box_bottom, width: sw, height: shadow_bottom - box_bottom },
                    color,
                    clip,
                );
            }
            // left band: border_box の高さ範囲で左側
            let band_top = by.max(sy);
            let band_bottom = box_bottom.min(shadow_bottom);
            if band_bottom > band_top {
                if sx < bx {
                    canvas.fill_rect_clipped(
                        Rect { x: sx, y: band_top, width: bx - sx, height: band_bottom - band_top },
                        color,
                        clip,
                    );
                }
                // right band: border_box の高さ範囲で右側
                let box_right = bx + bw;
                let shadow_right = sx + sw;
                if shadow_right > box_right {
                    canvas.fill_rect_clipped(
                        Rect {
                            x: box_right,
                            y: band_top,
                            width: shadow_right - box_right,
                            height: band_bottom - band_top,
                        },
                        color,
                        clip,
                    );
                }
            }
        }
    } else {
        // blur あり: 影の領域を含む一時バッファに描画し、
        // border_box 部分のアルファをゼロにしてから blur を適用する。
        let margin = blur.ceil() as i32 + 1;
        let buf_x = (shadow_rect.x - margin as f32).floor() as i32;
        let buf_y = (shadow_rect.y - margin as f32).floor() as i32;
        let buf_w = (shadow_rect.width + margin as f32 * 2.0).ceil() as u32 + 2;
        let buf_h = (shadow_rect.height + margin as f32 * 2.0).ceil() as u32 + 2;

        if buf_w == 0 || buf_h == 0 {
            return;
        }

        let mut shadow_buf = Canvas::new(buf_w, buf_h);
        let local_rect = Rect {
            x: shadow_rect.x - buf_x as f32,
            y: shadow_rect.y - buf_y as f32,
            width: shadow_rect.width,
            height: shadow_rect.height,
        };
        // 影の shape を描画（border_radius あれば rounded）
        if let Some((tl, tr, br, bl)) = radii {
            shadow_buf.fill_rounded_rect(
                local_rect,
                Color::rgba(color.r, color.g, color.b, 255),
                tl + spread,
                tr + spread,
                br + spread,
                bl + spread,
                None,
            );
        } else {
            shadow_buf.fill_rect(local_rect, Color::rgba(color.r, color.g, color.b, 255));
        }

        // border_box に対応するバッファ内領域のアルファをゼロにする（要素内側を除外）
        let box_local = Rect {
            x: border_box.x - buf_x as f32,
            y: border_box.y - buf_y as f32,
            width: border_box.width,
            height: border_box.height,
        };
        if let Some(box_local_n) = normalize_rect(box_local) {
            let x0 = box_local_n.x.floor().max(0.0) as i32;
            let y0 = box_local_n.y.floor().max(0.0) as i32;
            let x1 = (box_local_n.x + box_local_n.width).ceil().min(buf_w as f32) as i32;
            let y1 = (box_local_n.y + box_local_n.height).ceil().min(buf_h as f32) as i32;
            for py in y0..y1 {
                for px in x0..x1 {
                    let idx = (py as u32 * buf_w + px as u32) as usize * 4;
                    shadow_buf.pixels[idx + 3] = 0;
                }
            }
        }

        // 簡易 box blur: 半径 r = ceil(blur) のボックスで 3 回適用
        let r = blur.ceil() as u32;
        shadow_buf.box_blur_alpha(r);
        shadow_buf.box_blur_alpha(r);
        shadow_buf.box_blur_alpha(r);

        // color.a を alpha のスケールとして合成
        let alpha_scale = color.a as f32 / 255.0;
        if alpha_scale < 1.0 {
            shadow_buf.multiply_alpha(alpha_scale);
        }

        // メインキャンバスに合成（clip 適用）
        canvas.composite_canvas_clipped(&shadow_buf, buf_x, buf_y, color.r, color.g, color.b, clip);
    }
}
