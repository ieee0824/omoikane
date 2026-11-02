//! Pure parsing primitives for WebVTT subtitle resources.

/// A parsed WebVTT cue, owned independently from the input resource.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub id: String,
    pub start_time: f64,
    pub end_time: f64,
    pub text: String,
    pub settings: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub id: String,
    pub width: f64,
    pub lines: f64,
    pub region_anchor: (f64, f64),
    pub viewport_anchor: (f64, f64),
    pub scroll_up: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub cues: Vec<Cue>,
    pub regions: Vec<Region>,
}

/// Parses a complete WebVTT resource into cues.
///
/// Invalid cue blocks are ignored, as required by WebVTT's cue collection
/// algorithm. A missing `WEBVTT` signature rejects the complete resource.
#[cfg(test)]
pub fn parse(input: &str) -> Result<Vec<Cue>, ParseError> {
    Ok(parse_document(input)?.cues)
}

/// Parses cues and region definitions from a complete WebVTT resource.
pub fn parse_document(input: &str) -> Result<Document, ParseError> {
    let normalized = input
        .strip_prefix('\u{feff}')
        .unwrap_or(input)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut lines = normalized.lines();
    let signature = lines.next().unwrap_or_default();
    if signature != "WEBVTT"
        && !signature.starts_with("WEBVTT ")
        && !signature.starts_with("WEBVTT\t")
    {
        return Err(ParseError::MissingSignature);
    }

    let mut document = Document {
        cues: Vec::new(),
        regions: Vec::new(),
    };
    let mut block = Vec::new();
    for line in lines.chain(std::iter::once("")) {
        if line.is_empty() {
            if block.first() == Some(&"REGION") {
                if let Some(region) = parse_region(&block[1..]) {
                    document.regions.push(region);
                }
            } else if let Some(cue) = parse_block(&block) {
                document.cues.push(cue);
            }
            block.clear();
        } else {
            block.push(line);
        }
    }
    Ok(document)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    MissingSignature,
}

fn parse_block(lines: &[&str]) -> Option<Cue> {
    if lines.is_empty()
        || matches!(lines[0], line if line == "STYLE" || line == "REGION" || line.starts_with("NOTE"))
    {
        return None;
    }
    let (id, timing_index) = if lines[0].contains("-->") {
        ("", 0)
    } else {
        (*lines.first()?, 1)
    };
    let timing = *lines.get(timing_index)?;
    let (start, remainder) = timing.split_once("-->")?;
    let remainder = remainder.trim_start();
    let end_token_len = remainder
        .find(char::is_whitespace)
        .unwrap_or(remainder.len());
    let (end, settings) = remainder.split_at(end_token_len);
    let start_time = parse_timestamp(start.trim())?;
    let end_time = parse_timestamp(end)?;
    if end_time < start_time {
        return None;
    }
    Some(Cue {
        id: id.to_owned(),
        start_time,
        end_time,
        text: lines.get(timing_index + 1..)?.join("\n"),
        settings: settings.trim().to_owned(),
    })
}

fn parse_region(lines: &[&str]) -> Option<Region> {
    let mut region = Region {
        id: String::new(),
        width: 100.0,
        lines: 3.0,
        region_anchor: (0.0, 100.0),
        viewport_anchor: (0.0, 100.0),
        scroll_up: false,
    };
    for line in lines {
        let (name, value) = line.split_once(':')?;
        match name.trim() {
            "id" => region.id = value.trim().to_owned(),
            "width" => region.width = parse_percentage(value)?,
            "lines" => region.lines = value.trim().parse().ok()?,
            "regionanchor" => region.region_anchor = parse_anchor(value)?,
            "viewportanchor" => region.viewport_anchor = parse_anchor(value)?,
            "scroll" => region.scroll_up = value.trim() == "up",
            _ => {}
        }
    }
    (!region.id.is_empty()).then_some(region)
}

fn parse_percentage(value: &str) -> Option<f64> {
    let number = value.trim().strip_suffix('%')?.parse::<f64>().ok()?;
    (number.is_finite() && (0.0..=100.0).contains(&number)).then_some(number)
}

fn parse_anchor(value: &str) -> Option<(f64, f64)> {
    let (x, y) = value.trim().split_once(',')?;
    Some((parse_percentage(x)?, parse_percentage(y)?))
}

fn parse_timestamp(value: &str) -> Option<f64> {
    let (whole, millis) = value.rsplit_once('.')?;
    if millis.len() != 3 || !millis.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let fields: Vec<_> = whole.split(':').collect();
    let (hours, minutes, seconds) = match fields.as_slice() {
        [minutes, seconds] => (
            0_u64,
            minutes.parse::<u64>().ok()?,
            seconds.parse::<u64>().ok()?,
        ),
        [hours, minutes, seconds] => (
            hours.parse::<u64>().ok()?,
            minutes.parse::<u64>().ok()?,
            seconds.parse::<u64>().ok()?,
        ),
        _ => return None,
    };
    if minutes > 59 || seconds > 59 {
        return None;
    }
    Some(
        (hours * 3600 + minutes * 60 + seconds) as f64
            + millis.parse::<u64>().ok()? as f64 / 1000.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_identified_multiline_cues_and_settings() {
        let cues = parse("\u{feff}WEBVTT\r\n\r\nintro\r\n00:01.250 --> 01:02.500 align:start line:90%\r\nHello\r\nworld\r\n").unwrap();
        assert_eq!(
            cues,
            vec![Cue {
                id: "intro".into(),
                start_time: 1.25,
                end_time: 62.5,
                text: "Hello\nworld".into(),
                settings: "align:start line:90%".into()
            }]
        );
    }

    #[test]
    fn ignores_metadata_and_malformed_cue_blocks() {
        let cues = parse("WEBVTT\n\nNOTE comment\nignored\n\nSTYLE\n::cue { color: red }\n\n00:70.000 --> 00:71.000\nbad\n\n01:00:00.000 --> 01:00:01.000\ngood\n").unwrap();
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "good");
    }

    #[test]
    fn rejects_resources_without_signature() {
        assert_eq!(
            parse("00:00.000 --> 00:01.000\ntext"),
            Err(ParseError::MissingSignature)
        );
    }

    #[test]
    fn parses_region_settings_as_owned_values() {
        let document = parse_document("WEBVTT\n\nREGION\nid:captions\nwidth:60%\nlines:4\nregionanchor:10%,90%\nviewportanchor:20%,80%\nscroll:up\n\n00:00.000 --> 00:01.000 region:captions\ntext\n").unwrap();
        assert_eq!(
            document.regions,
            vec![Region {
                id: "captions".into(),
                width: 60.0,
                lines: 4.0,
                region_anchor: (10.0, 90.0),
                viewport_anchor: (20.0, 80.0),
                scroll_up: true,
            }]
        );
        assert_eq!(document.cues[0].settings, "region:captions");
    }
}
