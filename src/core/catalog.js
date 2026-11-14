'use strict';
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { pathToFileURL, fileURLToPath } = require('url');

/** Fetch a URL (https:// or file://) as text. */
async function fetchText(url) {
  if (url.startsWith('file:')) return fs.readFileSync(fileURLToPath(url), 'utf8');
  const res = await fetch(url, { headers: { 'User-Agent': 'Tirgumim-Launcher' }, cache: 'no-store' });
  if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
  return res.text();
}

function validateCatalog(c) {
  if (!c || c.schema !== 1 || !Array.isArray(c.games)) throw new Error('Catalog format not recognised');
  for (const g of c.games) {
    if (!g.id || !/^[a-z0-9][a-z0-9._-]*$/i.test(g.id)) throw new Error(`Bad game id in catalog: ${g.id}`);
  }
  return c;
}

async function loadCatalog(url) {
  const text = await fetchText(url);
  const cat = validateCatalog(JSON.parse(text.replace(/^﻿/, '')));
  // resolve relative pack URLs against the catalog URL
  for (const g of cat.games) {
    if (g.pack && g.pack.url) g.pack.url = new URL(g.pack.url, url).href;
    if (g.poster) g.poster = new URL(g.poster, url).href;
  }
  return cat;
}

/** Download a pack into destDir. Returns the local path. Local file:// packs are used in place. */
async function downloadPack(url, destDir, onProgress = () => {}) {
  if (url.startsWith('file:')) return fileURLToPath(url);
  if (!url.startsWith('https://')) throw new Error('Packs must be downloaded over https');
  fs.mkdirSync(destDir, { recursive: true });
  const out = path.join(destDir, `${crypto.randomBytes(6).toString('hex')}.zip`);
  const res = await fetch(url, { headers: { 'User-Agent': 'Tirgumim-Launcher' } });
  if (!res.ok) throw new Error(`Download failed: HTTP ${res.status}`);
  const total = Number(res.headers.get('content-length')) || 0;
  const ws = fs.createWriteStream(out);
  let got = 0;
  try {
    for await (const chunk of res.body) {
      got += chunk.length;
      if (!ws.write(chunk)) await new Promise((r) => ws.once('drain', r));
      onProgress(total ? got / total : null);
    }
  } finally {
    await new Promise((r) => ws.end(r));
  }
  return out;
}

module.exports = { loadCatalog, downloadPack, pathToFileURL };
