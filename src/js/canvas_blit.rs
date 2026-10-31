//! Bounded one-to-one Canvas pixel transfer; JS retains general drawing state.
use super::*;

pub(super) struct ByteView {
    pub(super) buffer: JsArrayBuffer,
    pub(super) offset: usize,
    pub(super) length: usize,
}

impl ByteView {
    /// A copy of the viewed bytes.
    pub(super) fn copy(&self) -> Option<Vec<u8>> {
        let data = self.buffer.data()?;
        Some(data.get(self.offset..)?.get(..self.length)?.to_vec())
    }
}

/// The `Uint8Array` in `value`, or `None` for anything else.
pub(super) fn byte_view(
    value: Option<&JsValue>,
    context: &mut Context,
) -> JsResult<Option<ByteView>> {
    let Some(object) = value.and_then(JsValue::as_object) else {
        return Ok(None);
    };
    let Ok(view) = JsUint8Array::from_object(object.clone()) else {
        return Ok(None);
    };
    let offset = view.byte_offset(context)?;
    let length = view.byte_length(context)?;
    let buffer = view.buffer(context)?;
    let Some(buffer) = buffer
        .as_object()
        .and_then(|b| JsArrayBuffer::from_object(b.clone()).ok())
    else {
        return Ok(None);
    };
    Ok(Some(ByteView {
        buffer,
        offset,
        length,
    }))
}

struct Blit {
    source: (usize, usize),
    target: (usize, usize),
    crop: (usize, usize, usize, usize),
    destination: (i64, i64),
}

struct Region {
    sx: usize,
    sy: usize,
    dx: usize,
    dy: usize,
    width: usize,
    height: usize,
}

fn image_bytes(size: (usize, usize)) -> Option<usize> {
    size.0.checked_mul(size.1)?.checked_mul(4)
}

impl Blit {
    fn parse(args: &[JsValue]) -> Option<Self> {
        let mut numbers = [0.0; 10];
        for (index, number) in numbers.iter_mut().enumerate() {
            *number = args.get(index + 2)?.as_number()?;
            if !number.is_finite() || number.fract() != 0.0 {
                return None;
            }
            let limit = if index < 8 {
                u32::MAX as f64
            } else {
                9_007_199_254_740_991.0
            };
            if number.abs() > limit || (index < 8 && *number < 0.0) {
                return None;
            }
        }
        let source = (numbers[0] as usize, numbers[1] as usize);
        let target = (numbers[2] as usize, numbers[3] as usize);
        let crop = (
            numbers[4] as usize,
            numbers[5] as usize,
            numbers[6] as usize,
            numbers[7] as usize,
        );
        if crop.0 > source.0
            || crop.1 > source.1
            || crop.2 > source.0 - crop.0
            || crop.3 > source.1 - crop.1
        {
            return None;
        }
        image_bytes(source)?;
        image_bytes(target)?;
        Some(Self {
            source,
            target,
            crop,
            destination: (numbers[8] as i64, numbers[9] as i64),
        })
    }

    fn region(&self) -> Option<Region> {
        let (dx, dy) = self.destination;
        let left = dx.max(0).min(self.target.0 as i64);
        let top = dy.max(0).min(self.target.1 as i64);
        let right = (dx + self.crop.2 as i64).max(0).min(self.target.0 as i64);
        let bottom = (dy + self.crop.3 as i64).max(0).min(self.target.1 as i64);
        if left >= right || top >= bottom {
            return None;
        }
        Some(Region {
            sx: self.crop.0 + (left - dx) as usize,
            sy: self.crop.1 + (top - dy) as usize,
            dx: left as usize,
            dy: top as usize,
            width: (right - left) as usize,
            height: (bottom - top) as usize,
        })
    }
}

fn snapshot(
    view: &ByteView,
    blit: &Blit,
    region: &Region,
    deadline: Option<Instant>,
) -> JsResult<Option<Vec<u8>>> {
    let result = snapshot_inner(view, blit, region, deadline);
    check_deadline(deadline)?;
    Ok(result)
}

fn check_deadline(deadline: Option<Instant>) -> JsResult<()> {
    if deadline.is_some_and(|limit| Instant::now() >= limit) {
        return Err(wall_clock_timeout_error());
    }
    Ok(())
}

fn snapshot_inner(
    view: &ByteView,
    blit: &Blit,
    region: &Region,
    deadline: Option<Instant>,
) -> Option<Vec<u8>> {
    let data = view.buffer.data()?;
    let pixels = data.get(view.offset..)?.get(..view.length)?;
    let mut snapshot = Vec::new();
    snapshot
        .try_reserve_exact(image_bytes((region.width, region.height))?)
        .ok()?;
    for y in region.sy..region.sy + region.height {
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            return None;
        }
        let start = (y * blit.source.0 + region.sx) * 4;
        snapshot.extend_from_slice(pixels.get(start..start + region.width * 4)?);
    }
    Some(snapshot)
}

fn blend(target: &mut [u8], source: &[u8]) {
    if source[3] == 255 {
        target.copy_from_slice(source);
        return;
    }
    if source[3] == 0 {
        if target[3] == 0 {
            target.fill(0);
        }
        return;
    }
    let sa = f64::from(source[3]) / 255.0;
    let da = f64::from(target[3]) / 255.0;
    let oa = sa + da * (1.0 - sa);
    for channel in 0..3 {
        target[channel] =
            ((f64::from(source[channel]) * sa + f64::from(target[channel]) * da * (1.0 - sa)) / oa)
                .round() as u8;
    }
    target[3] = (oa * 255.0).round() as u8;
}

/// Copies only the visible rectangle. Buffers are borrowed only after all JS
/// metadata has been read; the source snapshot makes overlapping views safe.
pub(super) fn native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let deadline = context.runtime_limits().deadline();
    check_deadline(deadline)?;
    let Some(blit) = Blit::parse(args) else {
        return Ok(false.into());
    };
    let Some(source) = byte_view(args.first(), context)? else {
        return Ok(false.into());
    };
    let Some(target) = byte_view(args.get(1), context)? else {
        return Ok(false.into());
    };
    if image_bytes(blit.source).is_none_or(|length| length > source.length)
        || image_bytes(blit.target).is_none_or(|length| length > target.length)
    {
        return Ok(false.into());
    }
    let Some(region) = blit.region() else {
        return Ok(true.into());
    };
    let Some(source) = snapshot(&source, &blit, &region, deadline)? else {
        return Ok(false.into());
    };
    {
        let Some(mut data) = target.buffer.data_mut() else {
            return Ok(false.into());
        };
        let Some(pixels) = data
            .get_mut(target.offset..)
            .and_then(|tail| tail.get_mut(..target.length))
        else {
            return Ok(false.into());
        };
        for y in 0..region.height {
            if deadline.is_some_and(|limit| Instant::now() >= limit) {
                break;
            }
            for x in 0..region.width {
                let from = (y * region.width + x) * 4;
                let to = ((region.dy + y) * blit.target.0 + region.dx + x) * 4;
                blend(&mut pixels[to..to + 4], &source[from..from + 4]);
            }
        }
    }
    check_deadline(deadline)?;
    Ok(true.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_deadline_remains_an_uncatchable_runtime_limit() {
        let mut context = Context::default();
        context
            .runtime_limits_mut()
            .set_deadline(Some(Instant::now()));
        let error = native(&JsValue::undefined(), &[], &mut context).unwrap_err();
        assert!(is_wall_clock_timeout(&error));
    }

    #[test]
    fn unsupported_geometry_falls_back_without_integer_saturation() {
        let mut args = vec![JsValue::undefined(); 12];
        for (index, value) in [
            56.0, 1120.0, 224.0, 680.0, 0.0, 0.0, 56.0, 1120.0, 0.0, -220.0,
        ]
        .into_iter()
        .enumerate()
        {
            args[index + 2] = value.into();
        }
        let blit = Blit::parse(&args).unwrap();
        let region = blit.region().unwrap();
        assert_eq!(
            (
                region.sx,
                region.sy,
                region.dx,
                region.dy,
                region.width,
                region.height
            ),
            (0, 220, 0, 0, 56, 680)
        );
        for (index, value) in [
            (10, 0.5),
            (11, f64::INFINITY),
            (2, 4_294_967_296.0),
            (6, 1.0),
        ] {
            let mut invalid = args.clone();
            invalid[index] = value.into();
            assert!(Blit::parse(&invalid).is_none());
        }
    }
}
