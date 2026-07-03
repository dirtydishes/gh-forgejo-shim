import { execFile } from "node:child_process";
import { promises as fs } from "node:fs";
import path from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const repoRoot = process.cwd();
const sourceDocsDir = path.resolve(repoRoot, process.argv[2] || "docs");
const outputDocsDir = path.resolve(repoRoot, process.argv[3] || sourceDocsDir);
const sourceDocsPath = toPosix(path.relative(repoRoot, sourceDocsDir)) || "docs";
const sourceBlobBase =
  process.env.DOCS_SOURCE_BLOB_BASE || "https://github.com/dirtydishes/gh-forgejo-shim/blob/main";
const localRepoFileUrlPattern = /file:\/\/\/[^"'<>\\\s)]+/g;
const rewriteExtensions = new Set([
  ".css",
  ".htm",
  ".html",
  ".js",
  ".json",
  ".md",
  ".svg",
  ".txt",
  ".xml",
  ".yml",
  ".yaml"
]);

function toPosix(value) {
  return value.split(path.sep).join(path.posix.sep);
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function encodeHref(value) {
  return value
    .split("/")
    .map((part) => {
      if (part === "" || part === "." || part === "..") {
        return part;
      }
      return encodeURIComponent(part);
    })
    .join("/");
}

function formatBytes(bytes) {
  if (bytes < 1024) {
    return `${bytes} B`;
  }

  const units = ["KB", "MB", "GB"];
  let size = bytes / 1024;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }

  return `${size.toFixed(size >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}

function formatDate(date) {
  return `${date.toISOString().slice(0, 16).replace("T", " ")} UTC`;
}

function dirname(relativePath) {
  const dir = path.posix.dirname(relativePath);
  return dir === "." ? "" : dir;
}

function isIgnored(relativePath) {
  return path.posix.basename(relativePath) === ".DS_Store";
}

function fileHref(currentDir, targetPath) {
  const from = currentDir || ".";
  let href = path.posix.relative(from, targetPath);
  if (!href.startsWith(".") && !href.startsWith("/")) {
    href = `./${href}`;
  }
  return encodeHref(href);
}

function encodePathSegments(value) {
  return value.split("/").map(encodeURIComponent).join("/");
}

function rewriteLocalRepoFileUrl(rawUrl, currentDir) {
  let parsed;

  try {
    parsed = new URL(rawUrl);
  } catch {
    return rawUrl;
  }

  const marker = "/gh-forgejo-shim";
  let decodedPath;

  try {
    decodedPath = decodeURIComponent(parsed.pathname);
  } catch {
    return rawUrl;
  }

  const markerIndex = decodedPath.indexOf(marker);

  if (markerIndex === -1) {
    return rawUrl;
  }

  const repoRelativePath = decodedPath.slice(markerIndex + marker.length).replace(/^\/+/, "");
  const suffix = `${parsed.search}${parsed.hash}`;

  if (!repoRelativePath) {
    return `https://github.com/dirtydishes/gh-forgejo-shim${suffix}`;
  }

  if (repoRelativePath === "docs" || repoRelativePath === "docs/") {
    return `${directoryHref(currentDir, "")}${suffix}`;
  }

  if (repoRelativePath.startsWith("docs/")) {
    return `${fileHref(currentDir, repoRelativePath.replace(/^docs\//, ""))}${suffix}`;
  }

  return `${sourceBlobBase.replace(/\/$/, "")}/${encodePathSegments(repoRelativePath)}${suffix}`;
}

function directoryHref(currentDir, targetDir) {
  const from = currentDir || ".";
  const target = targetDir || ".";
  let href = path.posix.relative(from, target);
  if (href === "") {
    href = ".";
  }
  if (!href.endsWith("/")) {
    href = `${href}/`;
  }
  if (!href.startsWith(".") && !href.startsWith("/")) {
    href = `./${href}`;
  }
  return encodeHref(href);
}

async function gitOutput(args) {
  try {
    const { stdout } = await execFileAsync("git", args, {
      cwd: repoRoot,
      maxBuffer: 10 * 1024 * 1024
    });
    return stdout;
  } catch {
    return "";
  }
}

async function walkFiles(rootDir, currentDir = "") {
  const directory = path.join(rootDir, ...currentDir.split("/").filter(Boolean));
  const entries = await fs.readdir(directory, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const relativePath = currentDir ? path.posix.join(currentDir, entry.name) : entry.name;

    if (entry.isDirectory()) {
      files.push(...(await walkFiles(rootDir, relativePath)));
      continue;
    }

    if (entry.isFile()) {
      files.push(relativePath);
    }
  }

  return files;
}

async function getDocsFiles() {
  const stdout = await gitOutput(["ls-files", "-z", "--", sourceDocsPath]);

  if (stdout.trim().length === 0) {
    return (await walkFiles(sourceDocsDir))
      .filter((relativePath) => !isIgnored(relativePath))
      .sort((a, b) => a.localeCompare(b));
  }

  return stdout
    .split("\0")
    .filter(Boolean)
    .map((filePath) => filePath.replace(new RegExp(`^${sourceDocsPath}/?`), ""))
    .filter((relativePath) => relativePath.length > 0 && !isIgnored(relativePath))
    .sort((a, b) => a.localeCompare(b));
}

async function getLastCommitDate(relativePath, fallbackDate) {
  const gitPath = path.posix.join(sourceDocsPath, relativePath);
  const stdout = await gitOutput(["log", "-1", "--format=%cI", "--", gitPath]);
  const trimmed = stdout.trim();
  return trimmed.length > 0 ? new Date(trimmed) : fallbackDate;
}

async function collectItems(files) {
  const items = [];

  for (const relativePath of files) {
    const absolutePath = path.join(sourceDocsDir, ...relativePath.split("/"));
    const stats = await fs.stat(absolutePath);

    items.push({
      relativePath,
      directory: dirname(relativePath),
      name: path.posix.basename(relativePath),
      extension: path.posix.extname(relativePath).replace(".", "") || "file",
      sizeBytes: stats.size,
      modifiedAt: await getLastCommitDate(relativePath, stats.mtime)
    });
  }

  return items;
}

function collectDirectories(items) {
  const directories = new Set([""]);

  for (const item of items) {
    if (!item.directory) {
      continue;
    }

    const parts = item.directory.split("/");
    for (let index = 1; index <= parts.length; index += 1) {
      directories.add(parts.slice(0, index).join("/"));
    }
  }

  return [...directories].sort((a, b) => {
    const depthDelta = a.split("/").length - b.split("/").length;
    return depthDelta === 0 ? a.localeCompare(b) : depthDelta;
  });
}

function directChildDirectories(currentDir, directories, items) {
  const prefix = currentDir ? `${currentDir}/` : "";
  const seen = new Set();
  const children = [];

  for (const directory of directories) {
    if (!directory || !directory.startsWith(prefix) || directory === currentDir) {
      continue;
    }

    const remainder = directory.slice(prefix.length);
    if (!remainder || remainder.includes("/")) {
      continue;
    }

    if (seen.has(directory)) {
      continue;
    }

    seen.add(directory);
    children.push({
      path: directory,
      name: remainder,
      fileCount: items.filter((item) => item.relativePath.startsWith(`${directory}/`)).length
    });
  }

  return children.sort((a, b) => a.name.localeCompare(b.name));
}

function directFiles(currentDir, items) {
  return items
    .filter((item) => item.directory === currentDir && item.name !== "index.html")
    .sort((a, b) => a.name.localeCompare(b.name));
}

function descendantFiles(currentDir, items) {
  if (!currentDir) {
    return items.filter((item) => item.name !== "index.html");
  }

  return items.filter(
    (item) => item.name !== "index.html" && item.relativePath.startsWith(`${currentDir}/`)
  );
}

function breadcrumbs(currentDir) {
  const crumbs = [{ label: "docs", path: "" }];
  if (!currentDir) {
    return crumbs;
  }

  const parts = currentDir.split("/");
  for (let index = 1; index <= parts.length; index += 1) {
    crumbs.push({
      label: parts[index - 1],
      path: parts.slice(0, index).join("/")
    });
  }
  return crumbs;
}

function renderBreadcrumbs(currentDir) {
  return breadcrumbs(currentDir)
    .map((crumb, index, list) => {
      const label = escapeHtml(crumb.label);
      if (index === list.length - 1) {
        return `<span>${label}</span>`;
      }
      return `<a href="${directoryHref(currentDir, crumb.path)}">${label}</a>`;
    })
    .join('<span class="separator">/</span>');
}

function renderDirectoryRows(currentDir, directories, items) {
  return directories
    .map((directory) => {
      const searchable = `${directory.path} ${directory.name} folder directory`.toLowerCase();
      return `<li class="entry" data-search="${escapeHtml(searchable)}">
        <a class="entry-link" href="${directoryHref(currentDir, directory.path)}">
          <span class="kind">dir</span>
          <span class="entry-main">${escapeHtml(directory.name)}/</span>
          <span class="meta">${directory.fileCount} files</span>
        </a>
      </li>`;
    })
    .join("\n");
}

function renderFileRows(currentDir, files, { showPath = false } = {}) {
  return files
    .map((file) => {
      const label = showPath ? file.relativePath : file.name;
      const searchable = `${file.relativePath} ${file.extension}`.toLowerCase();
      return `<li class="entry" data-search="${escapeHtml(searchable)}">
        <a class="entry-link" href="${fileHref(currentDir, file.relativePath)}">
          <span class="kind">${escapeHtml(file.extension)}</span>
          <span class="entry-main">${escapeHtml(label)}</span>
          <span class="meta">${escapeHtml(formatBytes(file.sizeBytes))} | ${escapeHtml(
            formatDate(file.modifiedAt)
          )}</span>
        </a>
      </li>`;
    })
    .join("\n");
}

function renderSection(title, rows, emptyText) {
  if (!rows) {
    return `<section class="section empty-section">
      <h2>${escapeHtml(title)}</h2>
      <p>${escapeHtml(emptyText)}</p>
    </section>`;
  }

  return `<section class="section">
    <h2>${escapeHtml(title)}</h2>
    <ul class="entries">${rows}</ul>
  </section>`;
}

function renderIndex(currentDir, context) {
  const { allDirectories, listItems } = context;
  const childDirectories = directChildDirectories(currentDir, allDirectories, listItems);
  const childFiles = directFiles(currentDir, listItems);
  const subtreeFiles = descendantFiles(currentDir, listItems).sort((a, b) =>
    a.relativePath.localeCompare(b.relativePath)
  );
  const title = currentDir || "docs";
  const folderRows = renderDirectoryRows(currentDir, childDirectories, listItems);
  const fileRows = renderFileRows(currentDir, childFiles);
  const allRows = renderFileRows(currentDir, subtreeFiles, { showPath: true });
  const shownCount = childDirectories.length + childFiles.length + subtreeFiles.length;
  const parentLink = currentDir
    ? `<a class="parent" href="${directoryHref(currentDir, dirname(currentDir))}">Parent folder</a>`
    : "";

  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>${escapeHtml(title)} - gh-forgejo-shim docs</title>
    <style>
      :root {
        color-scheme: dark;
        --bg: #101819;
        --surface: #162223;
        --surface-muted: #203032;
        --text: #e7f0ef;
        --muted: #a4b7b6;
        --border: #385052;
        --accent: #77d8cf;
        --accent-soft: #213d3d;
        --warn: #f0c66a;
      }

      * {
        box-sizing: border-box;
      }

      body {
        margin: 0;
        background: var(--bg);
        color: var(--text);
        font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        line-height: 1.55;
      }

      main {
        width: min(1120px, calc(100% - 32px));
        margin: 0 auto;
        padding: 32px 0 48px;
      }

      a {
        color: var(--accent);
        text-decoration-color: color-mix(in srgb, var(--accent) 50%, transparent);
        text-underline-offset: 0.18em;
      }

      .topline,
      .breadcrumbs,
      .toolbar,
      .entry-link {
        display: flex;
        align-items: center;
      }

      .topline {
        justify-content: space-between;
        gap: 16px;
        margin-bottom: 20px;
      }

      .brand,
      .parent {
        font-size: 0.92rem;
        color: var(--muted);
      }

      .breadcrumbs {
        flex-wrap: wrap;
        gap: 8px;
        color: var(--muted);
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
        font-size: 0.9rem;
      }

      .breadcrumbs span:last-child {
        color: var(--text);
      }

      .separator {
        color: var(--border);
      }

      h1 {
        margin: 10px 0 0;
        font-size: 2.2rem;
        line-height: 1.1;
        letter-spacing: 0;
      }

      .subtitle {
        max-width: 68ch;
        margin: 12px 0 0;
        color: var(--muted);
      }

      .toolbar {
        margin-top: 24px;
        padding: 14px;
        gap: 12px;
        border: 1px solid var(--border);
        border-radius: 8px;
        background: var(--surface);
      }

      .stats {
        min-width: max-content;
        color: var(--muted);
        font-size: 0.92rem;
      }

      .search {
        width: 100%;
        min-width: 0;
        border: 1px solid var(--border);
        border-radius: 8px;
        background: #0c1314;
        color: var(--text);
        font: inherit;
        padding: 10px 12px;
      }

      .search:focus {
        outline: 2px solid color-mix(in srgb, var(--accent) 35%, transparent);
        border-color: var(--accent);
      }

      .content {
        display: grid;
        gap: 16px;
        margin-top: 18px;
      }

      .section {
        border: 1px solid var(--border);
        border-radius: 8px;
        background: var(--surface);
        padding: 14px;
      }

      .section.hidden {
        display: none;
      }

      .section h2 {
        margin: 0 0 10px;
        font-size: 1rem;
        letter-spacing: 0;
      }

      .section p {
        margin: 0;
        color: var(--muted);
      }

      .entries {
        display: grid;
        gap: 6px;
        margin: 0;
        padding: 0;
        list-style: none;
      }

      .entry.hidden {
        display: none;
      }

      .entry-link {
        min-height: 42px;
        gap: 10px;
        padding: 8px 10px;
        border-radius: 6px;
        color: var(--text);
        text-decoration: none;
      }

      .entry-link:hover {
        background: var(--surface-muted);
      }

      .kind {
        width: 46px;
        flex: 0 0 46px;
        border: 1px solid var(--border);
        border-radius: 6px;
        background: var(--accent-soft);
        color: var(--accent);
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
        font-size: 0.72rem;
        line-height: 1.7;
        text-align: center;
      }

      .entry-main {
        min-width: 0;
        flex: 1 1 auto;
        overflow-wrap: anywhere;
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
        font-size: 0.92rem;
      }

      .meta {
        flex: 0 0 auto;
        color: var(--muted);
        font-size: 0.82rem;
        white-space: nowrap;
      }

      .empty {
        display: none;
        margin-top: 18px;
        border: 1px dashed var(--border);
        border-radius: 8px;
        padding: 20px;
        color: var(--muted);
        text-align: center;
      }

      @media (max-width: 720px) {
        main {
          width: min(100% - 24px, 1120px);
          padding-top: 24px;
        }

        .topline,
        .toolbar,
        .entry-link {
          align-items: stretch;
          flex-direction: column;
        }

        .toolbar {
          gap: 10px;
        }

        .kind,
        .meta {
          width: fit-content;
          flex-basis: auto;
        }
      }
    </style>
  </head>
  <body>
    <main>
      <div class="topline">
        <a class="brand" href="${directoryHref(currentDir, "")}">gh-forgejo-shim docs</a>
        ${parentLink}
      </div>

      <nav class="breadcrumbs">${renderBreadcrumbs(currentDir)}</nav>
      <h1>${escapeHtml(title)}</h1>
      <p class="subtitle">Static files published from <code>docs/</code>. HTML opens as pages, Markdown opens as source, and every folder without its own index gets this browsable listing.</p>

      <section class="toolbar">
        <div class="stats"><strong id="visible-count">${shownCount}</strong> entries shown</div>
        <input id="doc-search" class="search" type="search" placeholder="Filter this folder..." autocomplete="off" />
      </section>

      <div class="content">
        ${renderSection("Folders", folderRows, "No child folders.")}
        ${renderSection("Files", fileRows, "No direct files.")}
        ${renderSection("All files in this tree", allRows, "No files in this tree.")}
      </div>
      <p class="empty" id="empty-state">No entries match that filter.</p>
    </main>

    <script>
      const searchInput = document.getElementById("doc-search");
      const entries = Array.from(document.querySelectorAll(".entry"));
      const sections = Array.from(document.querySelectorAll(".section"));
      const visibleCount = document.getElementById("visible-count");
      const emptyState = document.getElementById("empty-state");

      function applyFilter(query) {
        const normalized = query.trim().toLowerCase();
        let shown = 0;

        for (const entry of entries) {
          const searchable = entry.dataset.search || "";
          const visible = normalized.length === 0 || searchable.includes(normalized);
          entry.classList.toggle("hidden", !visible);
          if (visible) {
            shown += 1;
          }
        }

        for (const section of sections) {
          const hasVisibleEntries = section.querySelector(".entry:not(.hidden)") !== null;
          const isEmptySection = section.classList.contains("empty-section");
          section.classList.toggle("hidden", !isEmptySection && !hasVisibleEntries);
        }

        visibleCount.textContent = String(shown);
        emptyState.style.display = shown === 0 ? "block" : "none";
      }

      searchInput.addEventListener("input", () => applyFilter(searchInput.value));
      applyFilter("");
    </script>
  </body>
</html>
`;
}

async function writeIndexes(context) {
  let generatedCount = 0;

  for (const directory of context.allDirectories) {
    if (context.sourceIndexDirectories.has(directory)) {
      continue;
    }

    const outputDir = path.join(outputDocsDir, ...directory.split("/").filter(Boolean));
    await fs.mkdir(outputDir, { recursive: true });
    await fs.writeFile(path.join(outputDir, "index.html"), renderIndex(directory, context), "utf8");
    generatedCount += 1;
  }

  return generatedCount;
}

async function rewriteLocalFileUrls(items) {
  let rewrittenFiles = 0;

  for (const item of items) {
    const extension = path.extname(item.relativePath);
    if (!rewriteExtensions.has(extension)) {
      continue;
    }

    const outputFile = path.join(outputDocsDir, ...item.relativePath.split("/"));
    const original = await fs.readFile(outputFile, "utf8");
    const rewritten = original.replace(localRepoFileUrlPattern, (rawUrl) =>
      rewriteLocalRepoFileUrl(rawUrl, item.directory)
    );

    if (rewritten !== original) {
      await fs.writeFile(outputFile, rewritten, "utf8");
      rewrittenFiles += 1;
    }
  }

  return rewrittenFiles;
}

async function main() {
  const sourceFiles = await getDocsFiles();
  const items = await collectItems(sourceFiles);
  const sourceIndexDirectories = new Set(
    items.filter((item) => item.name === "index.html").map((item) => item.directory)
  );
  const listItems = items.filter((item) => item.name !== "index.html");
  const allDirectories = collectDirectories(items);
  const rewrittenFiles = await rewriteLocalFileUrls(listItems);
  const generatedCount = await writeIndexes({
    allDirectories,
    listItems,
    sourceIndexDirectories
  });

  console.log(
    `Generated ${generatedCount} docs indexes in ${outputDocsDir} for ${listItems.length} files.`
  );
  console.log(`Rewrote local file URLs in ${rewrittenFiles} files.`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
