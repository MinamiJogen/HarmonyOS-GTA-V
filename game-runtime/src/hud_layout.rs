//! Derive a mobile frontend configuration without editing imported game assets.
//!
//! All radar rectangles receive the same affine transform, including the mask,
//! blur and expanded radar. The game's radar movie continues to own GPS, blips
//! and player bars. No pixels are copied and no replacement HUD is drawn here.

use std::ops::Range;

const SCALE: f64 = 4.0 / 3.0;
const LEFT: f64 = 0.012;
const TOP: f64 = 0.018;
const ORIGINAL_LEFT: f64 = -0.0045;
const ORIGINAL_TOP: f64 = 0.002 - 0.188888;
const BIGMAP_LEFT: f64 = -0.003975;
const BIGMAP_TOP: f64 = 0.022 - 0.460416666;

fn section(text: &str, name: &str) -> Result<Range<usize>, String> {
    let opening = format!("<{name}");
    let start = text.find(&opening).ok_or_else(|| format!("missing {name} section"))?;
    let end = text[start..].find(&format!("</{name}>"))
        .map(|end| start + end).ok_or_else(|| format!("unterminated {name} section"))?;
    Ok(start..end)
}

/// This bounded scanner only accepts quoted attributes in an existing opening
/// tag. It does not interpret XML entities or emit any imported resource bytes.
fn attribute(tag: &str, wanted: &str) -> Result<Range<usize>, String> {
    let bytes = tag.as_bytes();
    let mut at = 1;
    while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'>' { at += 1; }
    while at < bytes.len() {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1; }
        if at >= bytes.len() || matches!(bytes[at], b'/' | b'>') { break; }
        let start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'=' { at += 1; }
        let key = &tag[start..at];
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1; }
        if bytes.get(at) != Some(&b'=') { return Err(format!("invalid {key} attribute")); }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1; }
        let quote = *bytes.get(at).ok_or("missing attribute value")?;
        if !matches!(quote, b'\'' | b'"') { return Err(format!("unquoted {key} attribute")); }
        at += 1;
        let value_start = at;
        while at < bytes.len() && bytes[at] != quote { at += 1; }
        if at == bytes.len() { return Err(format!("unterminated {key} attribute")); }
        if key == wanted { return Ok(value_start..at); }
        at += 1;
    }
    Err(format!("missing {wanted} attribute"))
}

fn find_tag(text: &str, parent: &str, element: &str, name: Option<&str>) -> Result<Range<usize>, String> {
    let bounds = section(text, parent)?;
    let content = &text[bounds.clone()];
    let opening = format!("<{element}");
    let mut at = 0;
    let mut found = None;
    while let Some(offset) = content[at..].find(&opening) {
        let start = at + offset;
        at = start + opening.len();
        if !content.as_bytes().get(at).is_some_and(u8::is_ascii_whitespace) { continue; }
        let end = content[at..].find('>').map(|end| at + end + 1).ok_or("unterminated opening tag")?;
        let tag = &content[start..end];
        at = end;
        if name.is_some_and(|name| attribute(tag, "name").map(|range| &tag[range] != name).unwrap_or(true)) { continue; }
        if found.is_some() { return Err(format!("duplicate {element} {name:?}")); }
        found = Some(bounds.start + start..bounds.start + end);
    }
    found.ok_or_else(|| format!("missing {element} {name:?}"))
}

fn set(tag: &mut String, key: &str, value: &str) -> Result<(), String> {
    let range = attribute(tag, key)?;
    tag.replace_range(range, value);
    Ok(())
}

fn number(tag: &str, key: &str) -> Result<f64, String> {
    let range = attribute(tag, key)?;
    let number: f64 = tag[range].parse().map_err(|_| format!("invalid {key} number"))?;
    if number.is_finite() { Ok(number) } else { Err(format!("non-finite {key}")) }
}

fn edit(text: &mut String, parent: &str, element: &str, name: Option<&str>, changes: &[(&str, String)]) -> Result<(), String> {
    let range = find_tag(text, parent, element, name)?;
    let mut tag = text[range.clone()].to_string();
    for (key, value) in changes { set(&mut tag, key, value)?; }
    text.replace_range(range, &tag);
    Ok(())
}

fn radar(text: &mut String, parent: &str, element: &str, name: &str) -> Result<(), String> {
    let range = find_tag(text, parent, element, Some(name))?;
    let tag = &text[range];
    if &tag[attribute(tag, "alignX")?] != "L" || &tag[attribute(tag, "alignY")?] != "B" {
        return Err(format!("unsupported {name} alignment"));
    }
    let (x, y, width, height) = (number(tag, "posX")?, number(tag, "posY")?, number(tag, "sizeX")?, number(tag, "sizeY")?);
    if width <= 0.0 || height <= 0.0 { return Err(format!("invalid {name} size")); }
    // Bottom-aligned rectangles use their bottom edge as the position. Convert
    // each to its top edge BEFORE changing the anchor; otherwise masks drift.
    // Expanded radar used to grow upwards from the screen bottom. Give that
    // entire group its own top origin, so expansion stays on screen at the top.
    let (original_left, original_top) = if name.starts_with("bigmap") { (BIGMAP_LEFT, BIGMAP_TOP) } else { (ORIGINAL_LEFT, ORIGINAL_TOP) };
    let x = LEFT + (x - original_left) * SCALE;
    let y = TOP + (y - height - original_top) * SCALE;
    edit(text, parent, element, Some(name), &[
        ("alignY", "T".into()), ("posX", format!("{x:.9}")), ("posY", format!("{y:.9}")),
        ("sizeX", format!("{:.9}", width * SCALE)), ("sizeY", format!("{:.9}", height * SCALE)),
    ])
}

pub fn derive(original: &[u8]) -> Result<Vec<u8>, String> {
    let mut text = std::str::from_utf8(original).map_err(|_| "frontend is not UTF-8")?.to_owned();
    // Refuse an unexpected base layout as a whole. A partial map/mask override
    // would be worse than retaining the original HUD on an unknown asset set.
    let base_range = find_tag(&text, "minimap", "data", Some("minimap"))?;
    let base = &text[base_range];
    for (key, expected) in [("posX", ORIGINAL_LEFT), ("posY", 0.002), ("sizeX", 0.150), ("sizeY", 0.188888)] {
        if (number(base, key)? - expected).abs() > 0.000001 { return Err(format!("unsupported minimap {key}")); }
    }
    for name in ["minimap", "minimap_mask", "minimap_blur", "bigmap", "bigmap_mask", "bigmap_blur"] {
        radar(&mut text, "minimap", "data", name)?;
    }
    radar(&mut text, "ScriptHud", "component", "HUD_PLAYER_SWITCH_ALERT")?;
    for name in ["HUD_HELP_TEXT", "HUD_FLOATING_HELP_TEXT_1", "HUD_FLOATING_HELP_TEXT_2"] {
        edit(&mut text, "Hud", "component", Some(name), &[
            ("posX", "0.240".into()), ("posY", "0.018".into()), ("sizeX", "0.340".into()),
        ])?;
    }
    // The feed owns its scrolling movie. Give it a bounded column alongside the
    // radar, leaving the floating-stick region and bottom subtitles available.
    edit(&mut text, "GameStream", "position", None, &[
        ("alignY", "T".into()), ("posX", "0.240".into()), ("posY", "0.018".into()),
        ("sizeX", "0.240".into()), ("sizeY", "0.300".into()),
    ])?;
    Ok(text.into_bytes())
}

/// Return the minimap's actual normalized rectangle from the active frontend
/// bytes. Touch hit testing must follow the same resource view as rendering.
pub fn minimap_rect(frontend: &[u8]) -> Result<[f64; 4], String> {
    let text=std::str::from_utf8(frontend).map_err(|_| "frontend is not UTF-8")?;
    let tag=&text[find_tag(text,"minimap","data",Some("minimap"))?];
    if &tag[attribute(tag,"alignX")?] != "L" {return Err("unsupported minimap horizontal anchor".into());}
    let (x,y,width,height)=(number(tag,"posX")?,number(tag,"posY")?,number(tag,"sizeX")?,number(tag,"sizeY")?);
    let top=match &tag[attribute(tag,"alignY")?] {"T"=>y,"B"=>1.0+y-height,_=>return Err("unsupported minimap vertical anchor".into())};
    if width<=0.0 || height<=0.0 {return Err("invalid minimap size".into());}
    Ok([x,top,width,height])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Vec<u8> {
        std::fs::read(std::env::var("MUGUET_TEST_FRONTEND").expect("set the original frontend path")).unwrap()
    }

    #[test]
    fn original_asset_relocates_all_radar_parts_and_preserves_aspect() {
        let source = source();
        let derived = derive(&source).unwrap();
        let (original, patched) = (std::str::from_utf8(&source).unwrap(), std::str::from_utf8(&derived).unwrap());
        for name in ["minimap", "minimap_mask", "minimap_blur", "bigmap", "bigmap_mask", "bigmap_blur"] {
            let a = &original[find_tag(original, "minimap", "data", Some(name)).unwrap()];
            let b = &patched[find_tag(patched, "minimap", "data", Some(name)).unwrap()];
            assert_eq!(&b[attribute(b, "alignY").unwrap()], "T");
            let sx = number(b, "sizeX").unwrap() / number(a, "sizeX").unwrap();
            let sy = number(b, "sizeY").unwrap() / number(a, "sizeY").unwrap();
            assert!((sx - sy).abs() < 0.00000002, "{name} aspect changed");
            let origin_top = if name.starts_with("bigmap") { BIGMAP_TOP } else { ORIGINAL_TOP };
            let top = TOP + (number(a, "posY").unwrap() - number(a, "sizeY").unwrap() - origin_top) * SCALE;
            assert!((number(b, "posY").unwrap() - top).abs() < 0.000000001, "{name} anchor drift");
            if name == "minimap" || name == "bigmap" {
                assert!((number(b, "posY").unwrap() - TOP).abs() < 0.000000001);
                assert!(number(b, "posY").unwrap() + number(b, "sizeY").unwrap() < 1.0);
            }
        }
        let main = &patched[find_tag(patched, "minimap", "data", Some("minimap")).unwrap()];
        assert!((number(main, "sizeX").unwrap() - 0.2).abs() < 0.000001);
        assert!(number(main, "sizeY").unwrap() + TOP < 0.3);
        for name in ["pausemap", "pausemap_mask", "gallery", "gallery_mask"] {
            assert_eq!(&original[find_tag(original, "minimap", "data", Some(name)).unwrap()], &patched[find_tag(patched, "minimap", "data", Some(name)).unwrap()]);
        }
        assert_eq!(&original[find_tag(original, "Hud", "component", Some("HUD_SUBTITLE_TEXT")).unwrap()], &patched[find_tag(patched, "Hud", "component", Some("HUD_SUBTITLE_TEXT")).unwrap()]);
    }

    #[test]
    fn unknown_or_incomplete_layout_is_rejected_whole() {
        let source = String::from_utf8(source()).unwrap();
        assert!(derive(source.replace("name=\"minimap_mask\"", "name=\"unknown_mask\"").as_bytes()).is_err());
        assert!(derive(source.replace("sizeX=\"0.150\"", "sizeX=\"0.250\"").as_bytes()).is_err());
        assert!(derive(source.replace("<data name=\"minimap_mask\"", "<data name=\"minimap\"").as_bytes()).is_err());
        assert!(derive(b"not XML").is_err());
    }
}
