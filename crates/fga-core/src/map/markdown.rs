//! `bodyHTML`: a body's GitHub-flavoured Markdown rendered and then sanitized, as GitHub serves it.
//!
//! Clients such as ChatGPT desktop insert `bodyHTML` into their own pages as trusted HTML, so the
//! sanitizer is the security boundary, not the renderer. comrak renders GFM (tables, task lists,
//! strikethrough, autolinks, fenced code) and passes raw HTML through, as GitHub does; ammonia then
//! keeps only an allowlist close to GitHub's. Bodies come from anyone who can comment, so every
//! tag, attribute and URL outside that list is removed, whatever the renderer emitted.

use ammonia::Builder;
use comrak::arena_tree::NodeEdge;
use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options, format_html, parse_document};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Bodies longer than this are shown as escaped paragraphs instead of rendered. GitHub caps a body
/// at 65,536 characters; a larger one only reaches the gateway from Forgejo.
const MAX_RENDERED_BYTES: usize = 64 * 1024;

/// The sanitizer's HTML parser does work for each tag in proportion to how deeply the open elements
/// nest, so nesting is what makes a body costly, not its length. A body whose Markdown nests deeper
/// than this (blockquotes, lists, emphasis) is shown as escaped paragraphs; ordinary writing stays
/// far below it.
const MAX_DEPTH: usize = 64;

/// Raw HTML adds nesting the Markdown tree doesn't show. Past this many raw tags, a body's raw HTML
/// is shown as text instead of passed to the sanitizer.
const MAX_RAW_TAGS: usize = 256;

/// A table row with fewer cells than its header is filled out to the header's width, so a short
/// body can still ask for millions of cells. Past this many possible cells, tables are read as
/// text. The bound is counted from the source: a table has at most one column more than the pipes
/// on its delimiter line, and at most as many rows as the body has lines.
const MAX_TABLE_CELLS: usize = 20_000;

/// Rendered HTML past this size isn't sanitized: the body is shown as escaped paragraphs. The
/// sanitizer's time grows faster than the HTML in some shapes (a run of `&`, each written
/// `&amp;`), so the cap is on what it reads, not on the body.
const MAX_RENDERED_HTML: usize = 256 * 1024;

/// What one node of a parsed body draws from the render budget, in bytes, for building and walking
/// it. A short body can parse into tens of thousands of nodes (a sparse table's filled-out cells)
/// and be escaped afterwards, so parsing is charged as well as rendering. A node costs about as
/// much time as sanitizing two bytes of HTML; this charges twice that.
const NODE_COST: usize = 4;

/// Tags GitHub keeps in rendered Markdown, apart from the ones it reserves for its own features.
const TAGS: &[&str] = &[
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "caption",
    "cite",
    "code",
    "dd",
    "del",
    "details",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "input",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "tt",
    "ul",
    "var",
    "wbr",
];

/// Attributes kept on any allowed tag.
const GENERIC_ATTRIBUTES: &[&str] = &["align", "dir", "lang", "title"];

/// Attributes kept on one tag. No `id`, `name`, `class`, `style`, `target` or event handler.
const TAG_ATTRIBUTES: &[(&str, &[&str])] = &[
    ("a", &["href"]),
    ("img", &["src", "alt", "width", "height"]),
    ("td", &["colspan", "rowspan"]),
    ("th", &["colspan", "rowspan"]),
    ("ol", &["start"]),
    ("details", &["open"]),
    ("pre", &["lang"]),
    ("time", &["datetime"]),
    ("input", &["checked", "disabled"]),
];

/// Link and citation schemes; relative URLs pass. Images allow only absolute http(s), below.
const URL_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// The sanitizer, keeping images only from `forge`, the forge's own web origin.
///
/// An image loads by itself in the viewer's client, with the viewer's cookies for its host. The
/// gateway can't know the client's origin, so an image anywhere else (the client's own API, a
/// service on the viewer's network, a tracker) is dropped; without a forge origin, every image is.
fn sanitizer(forge: Option<url::Origin>) -> Builder<'static> {
    let mut builder = Builder::empty();
    builder
        .tags(TAGS.iter().copied().collect())
        .generic_attributes(GENERIC_ATTRIBUTES.iter().copied().collect())
        .tag_attributes(
            TAG_ATTRIBUTES
                .iter()
                .map(|(tag, attributes)| (*tag, attributes.iter().copied().collect()))
                .collect::<HashMap<_, HashSet<_>>>(),
        )
        // Every input is a checkbox, as a task list item's is, whatever the source said: never a
        // text field. (One forced value only: ammonia writes forced values in hash order.)
        .set_tag_attribute_value("input", "type", "checkbox")
        .url_schemes(URL_SCHEMES.iter().copied().collect())
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(Some("nofollow noopener noreferrer"))
        .strip_comments(true)
        .clean_content_tags(["script", "style"].into_iter().collect())
        .attribute_filter(move |tag, attribute, value| {
            if tag == "img" && attribute == "src" {
                return absolute_http_url(value)
                    .filter(|url| forge.as_ref().is_some_and(|forge| url.origin() == *forge))
                    .map(|url| String::from(url).into());
            }
            Some(value.into())
        });
    builder
}

/// An image source as an absolute `http` or `https` URL, written out in full, parsed. `https:/path`,
/// `https:path` and `https:\\path` resolve against the page showing the image, so they aren't
/// absolute here.
fn absolute_http_url(value: &str) -> Option<url::Url> {
    let trimmed = value.trim_matches(|c: char| c.is_ascii_whitespace() || c.is_ascii_control());
    let lower = trimmed.get(..8).unwrap_or(trimmed).to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return None;
    }
    let url = url::Url::parse(trimmed).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    // GitHub's comments turn each newline into a line break.
    options.render.hardbreaks = true;
    options.render.github_pre_lang = true;
    // Raw HTML reaches the sanitizer, which decides what stays, as on GitHub.
    options.render.r#unsafe = true;
    options
}

/// A body as GitHub-shaped `bodyHTML`: rendered GFM, sanitized, keeping images only from `forge`,
/// the forge's public web URL. An empty body is empty.
pub fn body_html(body: &str, forge: Option<&url::Url>) -> String {
    body_html_within(body, forge, &AtomicUsize::new(usize::MAX))
}

/// [`body_html`], drawing its work from `budget`: once parsed, a body draws its length and
/// [`NODE_COST`] per node, and once rendered, its HTML's length, shown or not. The body that empties
/// the budget is escaped, and an empty budget escapes a body before parsing it, so once it runs out
/// the rest of a document costs almost nothing. A body too long to parse draws nothing.
pub fn body_html_within(body: &str, forge: Option<&url::Url>, budget: &AtomicUsize) -> String {
    let body = body.replace('\r', "");
    if body.trim().is_empty() {
        return String::new();
    }
    if body.len() > MAX_RENDERED_BYTES || budget.load(Ordering::Relaxed) == 0 {
        return escaped_paragraphs(&body);
    }
    let arena = Arena::new();
    let mut options = options();
    if table_cells_bound(&body) > MAX_TABLE_CELLS {
        options.extension.table = false;
    }
    let root = parse_document(&arena, &body, &options);
    let (depth, raw_tags, nodes) = shape(root);
    let parsed = body.len().saturating_add(nodes.saturating_mul(NODE_COST));
    if !draw(budget, parsed) || depth > MAX_DEPTH {
        return escaped_paragraphs(&body);
    }
    if raw_tags > MAX_RAW_TAGS {
        options.render.r#unsafe = false;
        options.render.escape = true;
    }
    let mut rendered = String::new();
    let failed = format_html(root, &options, &mut rendered).is_err();
    // The work is done whether or not the HTML is shown, so it is charged either way.
    if !draw(budget, rendered.len()) || failed || rendered.len() > MAX_RENDERED_HTML {
        return escaped_paragraphs(&body);
    }
    let forge = forge.map(url::Url::origin).filter(url::Origin::is_tuple);
    sanitizer(forge)
        .clean(&rendered)
        .to_string()
        .trim()
        .to_owned()
}

/// Takes `amount` from `budget`, or empties it when less is left; whether it all fit.
fn draw(budget: &AtomicUsize, amount: usize) -> bool {
    let left = budget
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
            Some(left.saturating_sub(amount))
        })
        .unwrap_or_default();
    amount <= left
}

/// At most how many cells the body's tables could hold, filled-out rows included.
fn table_cells_bound(body: &str) -> usize {
    let lines = body.lines().count();
    let widest = body
        .lines()
        .map(|line| line.matches('|').count())
        .max()
        .unwrap_or(0);
    (widest + 1).saturating_mul(lines)
}

/// How deeply the Markdown tree nests, how many tags its raw HTML holds at most, and how many nodes
/// it has.
fn shape<'a>(root: &'a AstNode<'a>) -> (usize, usize, usize) {
    let (mut depth, mut deepest, mut raw_tags, mut nodes) = (0usize, 0usize, 0usize, 0usize);
    for edge in root.traverse() {
        match edge {
            NodeEdge::Start(node) => {
                depth += 1;
                nodes += 1;
                deepest = deepest.max(depth);
                raw_tags += match &node.data.borrow().value {
                    NodeValue::HtmlBlock(block) => block.literal.matches('<').count(),
                    NodeValue::HtmlInline(html) => html.matches('<').count(),
                    _ => 0,
                };
            }
            NodeEdge::End(_) => depth -= 1,
        }
    }
    (deepest, raw_tags, nodes)
}

fn escaped_paragraphs(body: &str) -> String {
    let escaped = body
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    escaped
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| format!("<p>{}</p>", paragraph.replace('\n', "<br>\n")))
        .collect::<Vec<_>>()
        .join("\n")
}
