// Writes every GraphQL document a T3 Code build defines, one file per operation, as T3 Code sends it.
//
// Usage: node scripts/capture/t3code-documents.mjs <server bundle .mjs> <output dir> [prefixes]
//
// T3 Code's server is one bundle in the app's `app.asar`, under `apps/server/dist/`: extract the
// archive with `npx @electron/asar extract` and pass the `.mjs` file that defines
// `PullRequestCore`. Operations built with effect/graphql keep their document as a template; this
// joins it the way effect's `render` does. With `prefixes` (comma-separated, such as `_0_,_1_`),
// it renders each document as an alias batch, as `GraphQLResolver.aliased` sends one, with one
// slice per prefix. The script reads the bundle as text and runs none of it.
import fs from 'node:fs';

const [bundle, out, prefixList] = process.argv.slice(2);
if (!bundle || !out) {
  console.error('usage: t3code-documents.mjs <server bundle .mjs> <output dir> [prefixes]');
  process.exit(2);
}
const src = fs.readFileSync(bundle, 'utf8');
const prefixes = prefixList ? prefixList.split(',') : [''];
fs.mkdirSync(out, { recursive: true });

// effect/dist/internal/graphql.js: two printed pieces take a space only where two
// non-punctuators would touch, and every odd part of a body is a name to prefix.
const concat = (l, r) => (/[\w"-]$/.test(l) && /^[\w"-]/.test(r) ? `${l} ${r}` : l + r);
const concatAll = (pieces) => pieces.reduce(concat, '');
const renderParts = (parts, prefix) => parts.map((part, i) => (i % 2 === 1 ? prefix + part : part)).join('');
const isShared = (fragment) => fragment.body.length <= 1;
const render = (kind, name, t) =>
  concatAll([
    kind,
    name,
    t.variables.length === 0
      ? ''
      : `(${concatAll(prefixes.flatMap((p) => t.variables.map((v) => `$${p}${v.name}:${v.body}`)))})`,
    t.directives === undefined ? '' : renderParts(t.directives, ''),
    `{${concatAll(
      prefixes.flatMap((p) =>
        t.selections.map((s) => {
          const key = p === '' ? s.alias : `${p}${s.alias ?? s.name}`;
          return `${key === undefined ? '' : `${key}:`}${s.name ?? ''}${renderParts(s.body, p)}`;
        }),
      ),
    )}}`,
    ...t.fragments.flatMap((f) =>
      (isShared(f) ? [''] : prefixes).map((p) => concatAll(['fragment', p + f.name, renderParts(f.body, p)])),
    ),
  ]);

// The template is an object literal of strings and arrays; read it as JSON5-like text.
function literalEnd(s, start) {
  let depth = 0;
  let quote = null;
  for (let i = start; i < s.length; i++) {
    const c = s[i];
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = null;
    } else if (c === '"' || c === "'" || c === '`') quote = c;
    else if (c === '{') depth++;
    else if (c === '}' && --depth === 0) return i + 1;
  }
  throw new Error('unbalanced template');
}
// Quotes bare keys and drops trailing commas, outside strings only, so JSON.parse can read it.
function parseTemplate(text) {
  let json = '';
  for (let i = 0; i < text.length; ) {
    const c = text[i];
    if (c === '"') {
      let j = i + 1;
      while (text[j] !== '"') j += text[j] === '\\' ? 2 : 1;
      json += text.slice(i, j + 1);
      i = j + 1;
    } else if (/[A-Za-z_$]/.test(c)) {
      const word = /^[\w$]+/.exec(text.slice(i))[0];
      json += `"${word}"`;
      i += word.length;
    } else if (c === ',' && /^,\s*[}\]]/.test(text.slice(i))) {
      i++;
    } else {
      json += c;
      i++;
    }
  }
  return JSON.parse(json);
}

const operation = /(?:query\$\d*|mutation)\("([A-Za-z]+)", \{\s*document: (["{])/g;
let count = 0;
for (let m; (m = operation.exec(src)); ) {
  const kind = m[0].startsWith('query') ? 'query' : 'mutation';
  const start = m.index + m[0].length - 1;
  const document =
    m[2] === '{'
      ? render(kind, m[1], parseTemplate(src.slice(start, literalEnd(src, start))))
      : JSON.parse(src.slice(start, src.indexOf('"', start + 1) + 1));
  fs.writeFileSync(`${out}/${kind}-${m[1]}.graphql`, `${document}\n`);
  count++;
}
console.log(`${count} documents`);
