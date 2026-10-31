//! Canvas 2D drawing styles, paths and compositing (Issue #1292).

use omoikane::js::JsRuntime;

/// Evaluates `body` with a fresh 2D context `ctx` on a `canvas` of
/// `width × height`; `px(x, y)` returns a pixel as `"r,g,b,a"`.
fn check(width: u32, height: u32, body: &str) {
    let mut runtime = JsRuntime::new().unwrap();
    let script = format!(
        r#"(() => {{
          const canvas = document.createElement('canvas');
          canvas.width = {width}; canvas.height = {height};
          const ctx = canvas.getContext('2d');
          const px = (x, y) => Array.from(ctx.getImageData(x, y, 1, 1).data).join();
          {body}
        }})()"#
    );
    let result = runtime.eval(&script).unwrap();
    assert!(result.as_boolean() == Some(true), "{}", result.display());
}

#[test]
fn styles_parse_and_serialize_colors_and_ignore_invalid_values() {
    check(
        1,
        1,
        r#"
        ctx.fillStyle = 'rgba(0, 255, 0, 0.5)';
        if (ctx.fillStyle !== 'rgba(0, 255, 0, 0.5)') return 'rgba ' + ctx.fillStyle;
        ctx.fillStyle = '#fa0';
        if (ctx.fillStyle !== '#ffaa00') return 'hex ' + ctx.fillStyle;
        ctx.fillStyle = 'rgb(-1e39, 1e39, -1e39)';
        if (ctx.fillStyle !== '#00ff00') return 'clamped ' + ctx.fillStyle;
        ctx.fillStyle = '#fa0';
        ctx.fillStyle = 'not a color';
        if (ctx.fillStyle !== '#ffaa00') return 'invalid ' + ctx.fillStyle;
        ctx.lineWidth = -1; ctx.lineWidth = Infinity; ctx.globalAlpha = 2; ctx.lineCap = 'bogus';
        ctx.globalCompositeOperation = 'nope';
        if (ctx.lineWidth !== 1 || ctx.globalAlpha !== 1 || ctx.lineCap !== 'butt' ||
            ctx.globalCompositeOperation !== 'source-over') return 'attributes';
        ctx.miterLimit = 1.5;
        if (ctx.miterLimit !== 1.5) return 'miterLimit ' + typeof ctx.miterLimit;
        ctx.letterSpacing = '3px';
        return ctx.letterSpacing === '3px' && ctx.textAlign === 'start' &&
          ctx.imageSmoothingEnabled === true || 'text attributes';
        "#,
    );
}

#[test]
fn fill_rect_keeps_the_current_path_and_antialiases_edges() {
    check(
        10,
        10,
        r#"
        ctx.beginPath(); ctx.rect(0, 0, 2, 2);
        ctx.fillStyle = 'red'; ctx.fillRect(5, 5, 2, 2);
        ctx.fillStyle = 'blue'; ctx.fill();
        if (px(1, 1) !== '0,0,255,255' || px(6, 6) !== '255,0,0,255') return 'path ' + px(1, 1);
        ctx.fillStyle = 'black'; ctx.fillRect(3.5, 0, 1, 1);
        return px(3, 0) === '0,0,0,128' || 'antialiasing ' + px(3, 0);
        "#,
    );
}

#[test]
fn gradients_and_patterns_paint_fills() {
    check(
        100,
        10,
        r#"
        const gradient = ctx.createLinearGradient(0, 0, 100, 0);
        gradient.addColorStop(0, '#ff0000'); gradient.addColorStop(1, '#0000ff');
        ctx.fillStyle = gradient; ctx.fillRect(0, 0, 100, 10);
        if (ctx.fillStyle !== gradient || px(0, 5) !== '254,0,1,255' || px(99, 5) !== '1,0,254,255')
          return 'linear ' + px(0, 5) + ' ' + px(99, 5);
        try { gradient.addColorStop(2, 'red'); return 'offset'; } catch (e) { if (e.name !== 'IndexSizeError') return e.name; }
        try { gradient.addColorStop(0, 'bogus'); return 'color'; } catch (e) { if (e.name !== 'SyntaxError') return e.name; }
        try { ctx.createRadialGradient(0, 0, -1, 0, 0, 1); return 'radius'; } catch (e) { if (e.name !== 'IndexSizeError') return e.name; }
        const radial = ctx.createRadialGradient(50, 5, 0, 50, 5, 50);
        radial.addColorStop(0, 'white'); radial.addColorStop(1, 'black');
        ctx.fillStyle = radial; ctx.fillRect(0, 0, 100, 10);
        const center = ctx.getImageData(50, 5, 1, 1).data, edge = ctx.getImageData(0, 5, 1, 1).data;
        if (center[0] < 250 || edge[0] > 5) return 'radial ' + px(50, 5) + ' ' + px(0, 5);
        const tile = document.createElement('canvas'); tile.width = 2; tile.height = 1;
        const tileContext = tile.getContext('2d');
        tileContext.fillStyle = 'lime'; tileContext.fillRect(0, 0, 1, 1);
        const pattern = ctx.createPattern(tile, 'repeat-x');
        if (!(pattern instanceof CanvasPattern)) return 'pattern type';
        try { ctx.createPattern(tile, 'bogus'); return 'repetition'; } catch (e) { if (e.name !== 'SyntaxError') return e.name; }
        ctx.clearRect(0, 0, 100, 10);
        ctx.fillStyle = pattern; ctx.fillRect(0, 0, 100, 10);
        return px(2, 0) === '0,255,0,255' && px(3, 0) === '0,0,0,0' && px(2, 1) === '0,0,0,0' ||
          'pattern ' + px(2, 0) + ' ' + px(3, 0) + ' ' + px(2, 1);
        "#,
    );
}

#[test]
fn line_dash_round_rect_and_transforms() {
    check(
        20,
        20,
        r#"
        ctx.setLineDash([2, 2, 1]);
        if (ctx.getLineDash().join() !== '2,2,1,2,2,1') return 'dash ' + ctx.getLineDash();
        ctx.setLineDash([1, -1]);
        if (ctx.getLineDash().length !== 6) return 'negative dash';
        ctx.setLineDash([2, 2]); ctx.lineWidth = 2;
        ctx.beginPath(); ctx.moveTo(0, 1); ctx.lineTo(20, 1); ctx.stroke();
        if (px(0, 0) !== '0,0,0,255' || px(2, 0) !== '0,0,0,0') return 'dashed ' + px(2, 0);
        ctx.setLineDash([]);
        try { ctx.roundRect(0, 0, 10, 10, [1, 2, 3, 4, 5]); return 'radii count'; } catch (e) { if (!(e instanceof RangeError)) return e.name; }
        ctx.beginPath(); ctx.roundRect(4, 4, 12, 12, 6); ctx.fillStyle = 'red'; ctx.fill();
        if (px(10, 10) !== '255,0,0,255' || px(4, 4) !== '0,0,0,0') return 'roundRect ' + px(4, 4);
        ctx.translate(3, 4); ctx.scale(2, 2);
        const matrix = ctx.getTransform();
        if (!(matrix instanceof DOMMatrix) || [matrix.a, matrix.d, matrix.e, matrix.f].join() !== '2,2,3,4')
          return 'getTransform';
        ctx.setTransform(1, 0, 0, 1, NaN, 0);
        if (ctx.getTransform().a !== 2) return 'non-finite setTransform';
        ctx.reset();
        return ctx.getTransform().isIdentity && px(10, 10) === '0,0,0,0' && ctx.fillStyle === '#000000' ||
          'reset';
        "#,
    );
}

#[test]
fn path2d_and_point_queries() {
    check(
        20,
        20,
        r#"
        const path = new Path2D('M 2 2 h 10 v 10 h -10 z');
        if (!ctx.isPointInPath(path, 5, 5) || ctx.isPointInPath(path, 15, 15)) return 'path2d point';
        ctx.translate(5, 0);
        if (ctx.isPointInPath(path, 3, 5) || !ctx.isPointInPath(path, 15, 5)) return 'transformed point';
        ctx.resetTransform();
        ctx.lineWidth = 4;
        if (!ctx.isPointInStroke(path, 2, 7) || ctx.isPointInStroke(path, 7, 7)) return 'stroke point';
        const copy = new Path2D(path);
        copy.addPath(new Path2D('M 14 14 h 4 v 4 h -4 z'));
        ctx.fillStyle = 'lime'; ctx.fill(copy);
        if (px(6, 6) !== '0,255,0,255' || px(16, 16) !== '0,255,0,255' || px(13, 13) !== '0,0,0,0')
          return 'path2d fill ' + px(16, 16);
        try { ctx.fill(path, 'bogus'); return 'fill rule'; } catch (e) { if (!(e instanceof TypeError)) return e.name; }
        ctx.beginPath(); ctx.arc(10, 10, 5, 0, Math.PI * 2, true);
        if (!ctx.isPointInPath(10, 10)) return 'anticlockwise full circle';
        ctx.beginPath(); ctx.arc(10, 10, 5, 0, Math.PI * 2);
        return ctx.isPointInPath(10, 10) && !ctx.isPointInPath(10, 16) && !ctx.isPointInPath(NaN, 10) || 'arc';
        "#,
    );
}

#[test]
fn composite_operations_shadows_and_draw_image() {
    check(
        20,
        20,
        r#"
        ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 20, 20);
        ctx.globalCompositeOperation = 'destination-out';
        ctx.fillRect(0, 0, 10, 20);
        if (px(5, 5) !== '0,0,0,0' || px(15, 5) !== '255,0,0,255') return 'destination-out';
        ctx.globalCompositeOperation = 'copy';
        ctx.fillStyle = 'blue'; ctx.fillRect(0, 0, 5, 5);
        if (px(2, 2) !== '0,0,255,255' || px(15, 15) !== '0,0,0,0') return 'copy ' + px(15, 15);
        ctx.globalCompositeOperation = 'source-over';
        ctx.clearRect(0, 0, 20, 20);
        ctx.shadowColor = 'lime'; ctx.shadowOffsetX = 5; ctx.shadowOffsetY = 5;
        if (ctx.shadowColor !== '#00ff00') return 'shadowColor';
        ctx.fillStyle = 'black'; ctx.fillRect(0, 0, 5, 5);
        if (px(2, 2) !== '0,0,0,255' || px(7, 7) !== '0,255,0,255') return 'shadow ' + px(7, 7);
        ctx.shadowColor = 'transparent';
        const source = document.createElement('canvas'); source.width = 2; source.height = 2;
        source.getContext('2d').fillStyle = 'blue';
        source.getContext('2d').fillRect(0, 0, 1, 1);
        ctx.clearRect(0, 0, 20, 20);
        ctx.imageSmoothingEnabled = false;
        ctx.drawImage(source, 0, 0, 2, 2, 10, 10, 8, 8);
        if (px(11, 11) !== '0,0,255,255' || px(15, 15) !== '0,0,0,0') return 'scaled drawImage ' + px(11, 11);
        ctx.globalAlpha = 0.5;
        ctx.imageSmoothingEnabled = true;
        ctx.drawImage(source, -1, -1, 4, 4, 0, 0, 4, 4);
        return px(1, 1) === '0,0,255,128' && px(0, 0) === '0,0,0,0' || 'clipped drawImage ' + px(1, 1);
        "#,
    );
}
