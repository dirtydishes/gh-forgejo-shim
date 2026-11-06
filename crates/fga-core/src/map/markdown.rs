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
use std::sync::LazyLock;

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

/// Rendered HTML past this size isn't sanitized: the body is shown as escaped paragraphs.
const MAX_RENDERED_HTML: usize = 1024 * 1024;

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

static SANITIZER: LazyLock<Builder<'static>> = LazyLock::new(|| {
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
        .attribute_filter(|tag, attribute, value| {
            if tag == "img" && attribute == "src" {
                return absolute_http_url(value).map(Into::into);
            }
            Some(value.into())
        });
    builder
});

/// An image source as an absolute `http` or `https` URL with a host, written out in full and
/// parsed again, or nothing. `https:/path`, `https:path` and `https:\\path` resolve against the
/// page that shows the image, which would make the client request its own origin.
fn absolute_http_url(value: &str) -> Option<String> {
    let trimmed = value.trim_matches(|c: char| c.is_ascii_whitespace() || c.is_ascii_control());
    let lower = trimmed.get(..8).unwrap_or(trimmed).to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return None;
    }
    let url = url::Url::parse(trimmed).ok()?;
    let public = match url.host()? {
        url::Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            !domain.is_empty()
                && domain != "localhost"
                && !domain.ends_with(".localhost")
                && !domain.ends_with(".local")
        }
        url::Host::Ipv4(ip) => {
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast())
        }
        url::Host::Ipv6(ip) => {
            let mapped = ip.to_ipv4_mapped();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
                || mapped.is_some_and(|v4| {
                    v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
                }))
        }
    };
    (matches!(url.scheme(), "http" | "https") && public).then(|| url.into())
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

/// A body as GitHub-shaped `bodyHTML`: rendered GFM, sanitized. An empty body is empty.
pub fn body_html(body: &str) -> String {
    let body = body.replace('\r', "");
    if body.trim().is_empty() {
        return String::new();
    }
    if body.len() > MAX_RENDERED_BYTES {
        return escaped_paragraphs(&body);
    }
    let arena = Arena::new();
    let mut options = options();
    if table_cells_bound(&body) > MAX_TABLE_CELLS {
        options.extension.table = false;
    }
    let root = parse_document(&arena, &body, &options);
    let (depth, raw_tags) = shape(root);
    if depth > MAX_DEPTH {
        return escaped_paragraphs(&body);
    }
    if raw_tags > MAX_RAW_TAGS {
        options.render.r#unsafe = false;
        options.render.escape = true;
    }
    let mut rendered = String::new();
    if format_html(root, &options, &mut rendered).is_err() || rendered.len() > MAX_RENDERED_HTML {
        return escaped_paragraphs(&body);
    }
    SANITIZER.clean(&rendered).to_string().trim().to_owned()
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

/// How deeply the Markdown tree nests, and how many tags its raw HTML holds at most.
fn shape<'a>(root: &'a AstNode<'a>) -> (usize, usize) {
    let (mut depth, mut deepest, mut raw_tags) = (0usize, 0usize, 0usize);
    for edge in root.traverse() {
        match edge {
            NodeEdge::Start(node) => {
                depth += 1;
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
    (deepest, raw_tags)
}

/// A body as `bodyHTML` without rendering its Markdown: escaped paragraphs and line breaks.
pub fn escaped_body_html(body: &str) -> String {
    escaped_paragraphs(&body.replace('\r', ""))
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
