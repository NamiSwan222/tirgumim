'use strict';
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

function sha256File(file) {
  return new Promise((resolve, reject) => {
    const h = crypto.createHash('sha256');
    fs.createReadStream(file)
      .on('error', reject)
      .on('data', (d) => h.update(d))
      .on('end', () => resolve(h.digest('hex')));
  });
}

// Hashing a 500 MB game exe takes seconds, so remember hashes while size+mtime are unchanged.
const hashCache = new Map();
async function sha256FileCached(file) {
  const st = fs.statSync(file);
  const key = `${path.resolve(file).toLowerCase()}|${st.size}|${st.mtimeMs}`;
  if (hashCache.has(key)) return hashCache.get(key);
  const h = await sha256File(file);
  hashCache.set(key, h);
  return h;
}

function sha256Buffer(buf) {
  return crypto.createHash('sha256').update(buf).digest('hex');
}

/** Validate a pack-relative path and return it normalised with forward slashes. */
function safeRelPath(p) {
  if (typeof p !== 'string' || !p.trim()) throw new Error(`Invalid path: ${JSON.stringify(p)}`);
  const norm = p.replace(/\\/g, '/');
  if (norm.startsWith('/') || /^[a-zA-Z]:/.test(norm)) throw new Error(`Absolute path not allowed: ${p}`);
  const parts = norm.split('/');
  if (parts.some((s) => s === '..' || s === '')) throw new Error(`Unsafe path: ${p}`);
  return norm;
}

/** Join a validated relative path under a root, guaranteeing it stays inside. */
function resolveInside(root, rel) {
  const abs = path.resolve(root, ...safeRelPath(rel).split('/'));
  const rootAbs = path.resolve(root);
  if (abs !== rootAbs && !abs.startsWith(rootAbs + path.sep)) throw new Error(`Path escapes root: ${rel}`);
  return abs;
}

function exists(p) {
  try { fs.accessSync(p); return true; } catch { return false; }
}

function compareVersions(a, b) {
  const pa = String(a).split('.').map((n) => parseInt(n, 10) || 0);
  const pb = String(b).split('.').map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] || 0) - (pb[i] || 0);
    if (d) return d > 0 ? 1 : -1;
  }
  return 0;
}

function readJson(file, fallback) {
  try { return JSON.parse(fs.readFileSync(file, 'utf8').replace(/^﻿/, '')); } catch { return fallback; }
}

function writeJsonAtomic(file, data) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const tmp = `${file}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(data, null, 2), 'utf8');
  fs.renameSync(tmp, file);
}

module.exports = { sha256File, sha256FileCached, sha256Buffer, safeRelPath, resolveInside, exists, compareVersions, readJson, writeJsonAtomic };
