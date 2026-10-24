use super::*;

fn pixels(source: &str) -> Vec<u8> {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(source).unwrap();
    serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped()).unwrap()
}

#[test]
fn canvas_blit_applies_integral_translation_and_clips_to_destination() {
    let actual = pixels(
        r#"(()=>{
        const source=new OffscreenCanvas(3,1),s=source.getContext('2d');
        const image=s.createImageData(3,1);image.data.set([255,0,0,255,0,255,0,128,23,34,45,0]);s.putImageData(image,0,0);
        const target=new OffscreenCanvas(5,2),t=target.getContext('2d');
        t.translate(1,1);t.drawImage(source,0,0);
        return JSON.stringify(Array.from(t.getImageData(0,0,5,2).data));
    })()"#,
    );
    let mut expected = vec![0; 20];
    expected.extend([
        0, 0, 0, 0, 255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    assert_eq!(actual, expected);
}

#[test]
fn canvas_blit_self_copy_uses_a_source_snapshot() {
    assert_eq!(
        pixels(
            r#"(()=>{
        const c=new OffscreenCanvas(4,1),t=c.getContext('2d'),image=t.createImageData(4,1);
        image.data.set([255,0,0,255,0,255,0,255,0,0,255,255,255,255,255,255]);t.putImageData(image,0,0);
        t.drawImage(c,0,0,3,1,1,0,3,1);
        return JSON.stringify(Array.from(t.getImageData(0,0,4,1).data));
    })()"#
        ),
        [
            255, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255
        ]
    );
}

#[test]
fn canvas_blit_crops_source_and_negative_destination() {
    assert_eq!(
        pixels(
            r#"(()=>{
        const source=new OffscreenCanvas(3,1),s=source.getContext('2d'),image=s.createImageData(3,1);
        image.data.set([255,0,0,255,0,255,0,255,0,0,255,255]);s.putImageData(image,0,0);
        const target=new OffscreenCanvas(2,1),t=target.getContext('2d');t.fillStyle='white';t.fillRect(0,0,2,1);
        t.drawImage(source,1,0,2,1,-1,0,2,1);
        return JSON.stringify(Array.from(t.getImageData(0,0,2,1).data));
    })()"#
        ),
        [0, 0, 255, 255, 255, 255, 255, 255]
    );
}

#[test]
fn canvas_blit_preserves_source_over_rounding() {
    assert_eq!(
        pixels(
            r#"(()=>{
        const source=new OffscreenCanvas(1,1),s=source.getContext('2d'),image=s.createImageData(1,1);
        image.data.set([100,50,200,128]);s.putImageData(image,0,0);
        const target=new OffscreenCanvas(1,1),t=target.getContext('2d'),background=t.createImageData(1,1);
        background.data.set([20,100,40,64]);t.putImageData(background,0,0);t.drawImage(source,0,0);
        return JSON.stringify(Array.from(t.getImageData(0,0,1,1).data));
    })()"#
        ),
        [84, 60, 168, 160]
    );
}

#[test]
fn canvas_blit_respects_typed_view_offsets_and_guard_bytes() {
    assert_eq!(
        pixels(
            r#"(()=>{
        const source=new OffscreenCanvas(1,1),s=source.getContext('2d');
        const input=new Uint8ClampedArray([9,9,9,9,255,0,0,255,7,7,7,7]);
        s.__s.pixels=new Uint8ClampedArray(input.buffer,4,4);
        const target=new OffscreenCanvas(1,1),t=target.getContext('2d');
        const output=new Uint8ClampedArray([8,8,8,8,0,0,0,0,6,6,6,6]);
        t.__s.pixels=new Uint8ClampedArray(output.buffer,4,4);t.drawImage(source,0,0);
        return JSON.stringify(Array.from(output));
    })()"#
        ),
        [8, 8, 8, 8, 255, 0, 0, 255, 6, 6, 6, 6]
    );
}
