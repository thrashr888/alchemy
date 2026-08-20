//! Hand-rolled minimal .pptx writer (docs/RFC-note-export.md).
//!
//! A .pptx is a zip of OOXML parts; the deliberately tiny set here —
//! content types, package rels, presentation, one master + one blank
//! layout + one theme, and a plain text-box slide per entry — is everything
//! PowerPoint and Keynote need to open a deck. No placeholder inheritance,
//! no numbering XML, no crate: the two existing writers cover docx/xlsx and
//! the pptx crates on crates.io are abandoned or read-only, so a page of
//! literal XML beats an unmaintained dependency.
//!
//! Two note shapes land here, mirroring their front-end parsers exactly:
//! slide decks (SlideDeck.tsx `parseDeck`: front-matter block + `---`
//! separators) and flashcards (Flashcards.tsx `parseCards`: `**Front:**` /
//! `**Back:**` blocks). Flashcards become a question slide followed by an
//! answer slide — the deck IS the self-test: face the prompt full-screen,
//! advance to reveal, exactly how the cards are used in the app.

use anyhow::{Context, Result};
use std::io::{Cursor, Write};

// 16:9 slide in EMU (914400 per inch).
const SLIDE_W: i64 = 12_192_000;
const SLIDE_H: i64 = 6_858_000;

// ---- Note parsing (mirrors the front-end parsers) ---------------------------

/// One slide: optional title plus body lines with an indent level.
pub struct Slide {
    pub title: Option<String>,
    /// (indent level 0-4, text). Bulleted in the front-end markdown.
    pub lines: Vec<(u8, String)>,
    /// Statement slides (flashcard fronts) center their single run.
    pub centered: bool,
}

/// Strip the inline markdown the generators emit (same set the print
/// sheets strip).
fn plain(text: &str) -> String {
    let mut s = text.replace("**", "");
    s = s.replace('`', "");
    // Lone emphasis stars; underscores stay (identifiers use them).
    s = s.replace('*', "");
    s.trim().to_string()
}

/// A front-matter line is `key: value` with a bare word key (parseDeck).
fn is_front_matter_line(line: &str) -> bool {
    let Some((key, value)) = line.split_once(':') else {
        return false;
    };
    !key.trim().is_empty()
        && key
            .trim()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !value.trim().is_empty()
}

/// A `---` slide separator on its own line (three or more dashes).
fn is_separator(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| c == '-')
}

/// Split markdown into `---`-separated blocks, like the front-end split.
fn blocks(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in md.lines() {
        if is_separator(line) {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push_str(line);
            cur.push('\n');
        }
    }
    out.push(cur);
    out
}

/// Slide-deck markdown → slides (mirrors SlideDeck.tsx `parseDeck`: skip the
/// front-matter block, one slide per `---` block, ≥2 slides or it isn't a
/// deck). Titles come from the block's first heading; bullets keep their
/// nesting; other lines are plain body text.
pub fn parse_deck(md: &str) -> Vec<Slide> {
    let mut slides = Vec::new();
    for block in blocks(md) {
        let text = block.trim();
        if text.is_empty() {
            continue;
        }
        if text.lines().all(|l| is_front_matter_line(l.trim())) {
            continue; // style front-matter, not a slide
        }
        let mut title = None;
        let mut lines = Vec::new();
        for raw in text.lines() {
            let t = raw.trim_end();
            if t.trim().is_empty() {
                continue;
            }
            let trimmed = t.trim_start();
            if trimmed.starts_with('#') && title.is_none() {
                title = Some(plain(trimmed.trim_start_matches('#')));
                continue;
            }
            let indent = (t.len() - trimmed.len()) as u8;
            if let Some(rest) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
                .or_else(|| trimmed.strip_prefix("• "))
            {
                lines.push(((indent / 2).min(4), format!("• {}", plain(rest))));
            } else {
                lines.push((0, plain(trimmed)));
            }
        }
        if title.is_some() || !lines.is_empty() {
            slides.push(Slide {
                title,
                lines,
                centered: false,
            });
        }
    }
    slides
}

/// Flashcard markdown → (front, back) pairs (mirrors Flashcards.tsx
/// `parseCards`: `---`-separated blocks each holding `**Front:** …
/// **Back:** …`; ≥2 cards or it isn't a deck).
pub fn parse_cards(md: &str) -> Vec<(String, String)> {
    let mut cards = Vec::new();
    for block in blocks(md) {
        let Some(front_at) = block.find("**Front:**") else {
            continue;
        };
        let Some(back_at) = block[front_at..].find("**Back:**").map(|i| i + front_at) else {
            continue;
        };
        let front = plain(&block[front_at + "**Front:**".len()..back_at]);
        let back = plain(&block[back_at + "**Back:**".len()..]);
        if !front.is_empty() && !back.is_empty() {
            cards.push((front, back));
        }
    }
    cards
}

/// Flashcards as a question-then-answer slide pair per card: the deck is
/// the self-test, so the prompt gets a full slide before the reveal.
pub fn cards_to_slides(cards: &[(String, String)]) -> Vec<Slide> {
    let mut slides = Vec::new();
    for (front, back) in cards {
        slides.push(Slide {
            title: None,
            lines: vec![(0, front.clone())],
            centered: true,
        });
        slides.push(Slide {
            title: Some(front.clone()),
            lines: back
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| (0, plain(l)))
                .collect(),
            centered: false,
        });
    }
    slides
}

// ---- OOXML writing ----------------------------------------------------------

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

const NS: &str = "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
     xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
     xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"";

const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>";

/// The empty group every spTree starts with.
const GROUP: &str =
    "<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
     <p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/>\
     <a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr>";

/// A plain text box at an explicit position — no placeholders, so nothing
/// depends on layout inheritance.
#[allow(clippy::too_many_arguments)]
fn text_box(
    id: u32,
    name: &str,
    x: i64,
    y: i64,
    cx: i64,
    cy: i64,
    anchor_middle: bool,
    paragraphs: &str,
) -> String {
    let anchor = if anchor_middle { " anchor=\"ctr\"" } else { "" };
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/>\
         <p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr>\
         <p:txBody><a:bodyPr wrap=\"square\"{anchor}><a:normAutofit/></a:bodyPr><a:lstStyle/>{paragraphs}</p:txBody></p:sp>"
    )
}

/// One paragraph: size in hundredths of a point, indent by level.
fn para(text: &str, size: u32, bold: bool, level: u8, centered: bool) -> String {
    let b = if bold { " b=\"1\"" } else { "" };
    let algn = if centered { " algn=\"ctr\"" } else { "" };
    let mar_l = i64::from(level) * 342_900;
    format!(
        "<a:p><a:pPr marL=\"{mar_l}\"{algn}/>\
         <a:r><a:rPr lang=\"en-US\" sz=\"{size}\"{b} dirty=\"0\">\
         <a:solidFill><a:srgbClr val=\"1A1A1A\"/></a:solidFill></a:rPr>\
         <a:t>{}</a:t></a:r></a:p>",
        esc(text)
    )
}

fn slide_xml(slide: &Slide) -> String {
    let mut shapes = String::new();
    let mut id = 2u32;
    if let Some(title) = &slide.title {
        shapes.push_str(&text_box(
            id,
            "Title",
            838_200,
            457_200,
            SLIDE_W - 2 * 838_200,
            1_143_000,
            false,
            &para(title, 3200, true, 0, false),
        ));
        id += 1;
    }
    if !slide.lines.is_empty() {
        let paras: String = slide
            .lines
            .iter()
            .map(|(level, text)| {
                let size = if slide.centered { 3200 } else { 1800 };
                para(text, size, slide.centered, *level, slide.centered)
            })
            .collect();
        let (y, cy) = if slide.title.is_some() {
            (1_800_000, SLIDE_H - 2_300_000)
        } else {
            (500_000, SLIDE_H - 1_000_000)
        };
        shapes.push_str(&text_box(
            id,
            "Body",
            838_200,
            y,
            SLIDE_W - 2 * 838_200,
            cy,
            slide.centered,
            &paras,
        ));
    }
    format!(
        "{XML_DECL}<p:sld {NS}><p:cSld><p:spTree>{GROUP}{shapes}</p:spTree></p:cSld>\
         <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
    )
}

/// Minimal but complete Office theme — every list the schema requires,
/// nothing else. Readers only need it to resolve default fonts and colors.
fn theme_xml() -> String {
    let fills = "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>\
         <a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>\
         <a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>";
    format!(
        "{XML_DECL}<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"Alchemy\">\
         <a:themeElements>\
         <a:clrScheme name=\"Alchemy\">\
         <a:dk1><a:srgbClr val=\"1A1A1A\"/></a:dk1><a:lt1><a:srgbClr val=\"FFFFFF\"/></a:lt1>\
         <a:dk2><a:srgbClr val=\"44546A\"/></a:dk2><a:lt2><a:srgbClr val=\"E7E6E6\"/></a:lt2>\
         <a:accent1><a:srgbClr val=\"4472C4\"/></a:accent1><a:accent2><a:srgbClr val=\"ED7D31\"/></a:accent2>\
         <a:accent3><a:srgbClr val=\"A5A5A5\"/></a:accent3><a:accent4><a:srgbClr val=\"FFC000\"/></a:accent4>\
         <a:accent5><a:srgbClr val=\"5B9BD5\"/></a:accent5><a:accent6><a:srgbClr val=\"70AD47\"/></a:accent6>\
         <a:hlink><a:srgbClr val=\"0563C1\"/></a:hlink><a:folHlink><a:srgbClr val=\"954F72\"/></a:folHlink>\
         </a:clrScheme>\
         <a:fontScheme name=\"Alchemy\">\
         <a:majorFont><a:latin typeface=\"Helvetica Neue\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont>\
         <a:minorFont><a:latin typeface=\"Helvetica Neue\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont>\
         </a:fontScheme>\
         <a:fmtScheme name=\"Alchemy\">\
         <a:fillStyleLst>{fills}</a:fillStyleLst>\
         <a:lnStyleLst>\
         <a:ln w=\"6350\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>\
         <a:ln w=\"12700\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>\
         <a:ln w=\"19050\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>\
         </a:lnStyleLst>\
         <a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle>\
         <a:effectStyle><a:effectLst/></a:effectStyle>\
         <a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>\
         <a:bgFillStyleLst>{fills}</a:bgFillStyleLst>\
         </a:fmtScheme>\
         </a:themeElements></a:theme>"
    )
}

/// Assemble the package. `slides` must be non-empty.
pub fn pptx_bytes(slides: &[Slide]) -> Result<Vec<u8>> {
    anyhow::ensure!(!slides.is_empty(), "the deck has no slides");
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let put = |z: &mut zip::ZipWriter<Cursor<Vec<u8>>>, name: &str, body: &str| -> Result<()> {
        z.start_file(name, opts)
            .with_context(|| format!("could not start {name}"))?;
        z.write_all(body.as_bytes())?;
        Ok(())
    };

    let n = slides.len();
    let overrides: String = (1..=n)
        .map(|i| {
            format!(
                "<Override PartName=\"/ppt/slides/slide{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>"
            )
        })
        .collect();
    put(&mut zip, "[Content_Types].xml", &format!(
        "{XML_DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/>\
         <Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml\"/>\
         <Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml\"/>\
         <Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>\
         {overrides}</Types>"
    ))?;

    put(&mut zip, "_rels/.rels", &format!(
        "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/>\
         </Relationships>"
    ))?;

    let slide_ids: String = (1..=n)
        .map(|i| format!("<p:sldId id=\"{}\" r:id=\"rId{}\"/>", 255 + i, 1 + i))
        .collect();
    put(
        &mut zip,
        "ppt/presentation.xml",
        &format!(
            "{XML_DECL}<p:presentation {NS}>\
         <p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst>\
         <p:sldIdLst>{slide_ids}</p:sldIdLst>\
         <p:sldSz cx=\"{SLIDE_W}\" cy=\"{SLIDE_H}\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/>\
         </p:presentation>"
        ),
    )?;

    let slide_rels: String = (1..=n)
        .map(|i| {
            format!(
                "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{i}.xml\"/>",
                1 + i
            )
        })
        .collect();
    put(&mut zip, "ppt/_rels/presentation.xml.rels", &format!(
        "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"slideMasters/slideMaster1.xml\"/>\
         {slide_rels}\
         <Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"theme/theme1.xml\"/>\
         </Relationships>",
        n + 2
    ))?;

    put(&mut zip, "ppt/slideMasters/slideMaster1.xml", &format!(
        "{XML_DECL}<p:sldMaster {NS}>\
         <p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>\
         <p:spTree>{GROUP}</p:spTree></p:cSld>\
         <p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" \
          accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/>\
         <p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rId1\"/></p:sldLayoutIdLst>\
         </p:sldMaster>"
    ))?;
    put(&mut zip, "ppt/slideMasters/_rels/slideMaster1.xml.rels", &format!(
        "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>\
         <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"../theme/theme1.xml\"/>\
         </Relationships>"
    ))?;

    put(
        &mut zip,
        "ppt/slideLayouts/slideLayout1.xml",
        &format!(
        "{XML_DECL}<p:sldLayout {NS}><p:cSld name=\"Blank\"><p:spTree>{GROUP}</p:spTree></p:cSld>\
         <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"
    ),
    )?;
    put(&mut zip, "ppt/slideLayouts/_rels/slideLayout1.xml.rels", &format!(
        "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"../slideMasters/slideMaster1.xml\"/>\
         </Relationships>"
    ))?;

    put(&mut zip, "ppt/theme/theme1.xml", &theme_xml())?;

    for (i, slide) in slides.iter().enumerate() {
        put(
            &mut zip,
            &format!("ppt/slides/slide{}.xml", i + 1),
            &slide_xml(slide),
        )?;
        put(&mut zip, &format!("ppt/slides/_rels/slide{}.xml.rels", i + 1), &format!(
            "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
             <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>\
             </Relationships>"
        ))?;
    }

    Ok(zip.finish()?.into_inner())
}

#[cfg(test)]
mod pptx_tests {
    use super::*;
    use std::io::Read;

    const DECK_MD: &str = "\
theme: graphite\nfont: sans\n\n---\n\n# The Rollout\n\nA pilot in three regions\n\n---\n\n## What worked\n\n- Fast onboarding\n  - One afternoon setup\n- Weekly briefs\n\n---\n\n## Next\n\n- Expand north\n";

    const CARDS_MD: &str = "\
**Front:** What is RAG?\n**Back:** Retrieval-augmented generation — ground answers in retrieved passages.\n\n---\n\n**Front:** What does BM25 rank by?\n**Back:** Term frequency and inverse document frequency.\n";

    fn parts(bytes: &[u8]) -> Vec<(String, String)> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        let mut out = Vec::new();
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).unwrap();
            let mut body = String::new();
            f.read_to_string(&mut body).unwrap();
            out.push((f.name().to_string(), body));
        }
        out
    }

    /// Every part is well-formed XML (quick-xml push parse to Eof).
    fn assert_well_formed(name: &str, xml: &str) {
        let mut reader = quick_xml::Reader::from_str(xml);
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(e) => panic!("{name} is not well-formed XML: {e}"),
            }
        }
    }

    #[test]
    fn deck_parses_like_the_frontend() {
        let slides = parse_deck(DECK_MD);
        assert_eq!(slides.len(), 3); // front-matter block skipped
        assert_eq!(slides[0].title.as_deref(), Some("The Rollout"));
        assert_eq!(slides[1].title.as_deref(), Some("What worked"));
        // Nested bullet keeps a deeper level.
        assert_eq!(slides[1].lines[0], (0, "• Fast onboarding".into()));
        assert_eq!(slides[1].lines[1], (1, "• One afternoon setup".into()));
    }

    #[test]
    fn cards_parse_like_the_frontend() {
        let cards = parse_cards(CARDS_MD);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].0, "What is RAG?");
        // Q slide then A slide per card.
        let slides = cards_to_slides(&cards);
        assert_eq!(slides.len(), 4);
        assert!(slides[0].centered && slides[0].title.is_none());
        assert_eq!(slides[1].title.as_deref(), Some("What is RAG?"));
    }

    #[test]
    fn package_has_every_required_part_and_valid_xml() {
        let bytes = pptx_bytes(&parse_deck(DECK_MD)).unwrap();
        assert_eq!(&bytes[..2], b"PK");
        let parts = parts(&bytes);
        let names: Vec<&str> = parts.iter().map(|(n, _)| n.as_str()).collect();
        for required in [
            "[Content_Types].xml",
            "_rels/.rels",
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            "ppt/slideLayouts/slideLayout1.xml",
            "ppt/theme/theme1.xml",
            "ppt/slides/slide1.xml",
            "ppt/slides/slide3.xml",
            "ppt/slides/_rels/slide1.xml.rels",
        ] {
            assert!(names.contains(&required), "missing {required}");
        }
        for (name, body) in &parts {
            assert_well_formed(name, body);
        }
        // Every slide override is declared and every slide has a rel entry.
        let (_, types) = parts
            .iter()
            .find(|(n, _)| n == "[Content_Types].xml")
            .unwrap();
        assert!(types.contains("/ppt/slides/slide3.xml"));
        let (_, rels) = parts
            .iter()
            .find(|(n, _)| n == "ppt/_rels/presentation.xml.rels")
            .unwrap();
        assert!(rels.contains("slides/slide3.xml") && rels.contains("theme/theme1.xml"));
    }

    #[test]
    fn text_is_escaped() {
        let slides = vec![Slide {
            title: Some("A < B & \"C\"".into()),
            lines: vec![(0, "x > y".into())],
            centered: false,
        }];
        let bytes = pptx_bytes(&slides).unwrap();
        let parts = parts(&bytes);
        let (_, slide) = parts
            .iter()
            .find(|(n, _)| n == "ppt/slides/slide1.xml")
            .unwrap();
        assert!(slide.contains("A &lt; B &amp; &quot;C&quot;"));
        assert_well_formed("slide1", slide);
    }

    /// Dev helper: emit sample decks to the temp dir for opening in
    /// Keynote/PowerPoint by hand. `cargo test --lib write_sample -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn write_sample_pptx_files() {
        let dir = std::env::temp_dir();
        let deck = dir.join("alchemy-sample-deck.pptx");
        std::fs::write(&deck, pptx_bytes(&parse_deck(DECK_MD)).unwrap()).unwrap();
        let cards = dir.join("alchemy-sample-cards.pptx");
        std::fs::write(
            &cards,
            pptx_bytes(&cards_to_slides(&parse_cards(CARDS_MD))).unwrap(),
        )
        .unwrap();
        println!("wrote {} and {}", deck.display(), cards.display());
    }

    #[test]
    fn non_deck_markdown_yields_no_slides() {
        assert!(parse_deck("").is_empty());
        assert!(parse_cards("Just prose.").is_empty());
    }
}
