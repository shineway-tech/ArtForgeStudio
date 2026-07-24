#!/usr/bin/env node

'use strict';

const fs = require('fs');
const path = require('path');

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (!item.startsWith('--')) continue;
    const key = item.slice(2);
    const value = argv[index + 1] && !argv[index + 1].startsWith('--')
      ? argv[index += 1]
      : 'true';
    args[key] = value;
  }
  return args;
}

function required(value, label) {
  if (!value) throw new Error(`Missing ${label}`);
  return value;
}

function normalizePrefix(prefix) {
  return String(prefix || 'public/artforge_studio')
    .replace(/^\/+|\/+$/g, '')
    .replace(/\/+/g, '/');
}

function publicUrl(baseUrl, prefix, fileName) {
  return `${baseUrl.replace(/\/+$/g, '')}/${prefix}/${fileName}`;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const version = required(args.version, '--version').replace(/^v/i, '');
  if (!/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error(`Invalid semantic version: ${version}`);
  }

  const output = path.resolve(required(args.output, '--output'));
  const baseUrl = process.env.ALIYUN_OSS_PUBLIC_BASE_URL || 'https://cdn.honeykid.cn';
  const prefix = normalizePrefix(process.env.ALIYUN_OSS_PREFIX);
  const notes = String(
    process.env.ARTFORGE_RELEASE_NOTES || '本次更新包含功能优化与问题修复。',
  ).trim();

  const manifest = {
    version,
    published_at: new Date().toISOString(),
    notes,
    downloads: {
      macos_aarch64: publicUrl(
        baseUrl,
        prefix,
        'ArtForgeStudio_macos_aarch64.dmg',
      ),
      macos_x64: publicUrl(baseUrl, prefix, 'ArtForgeStudio_macos_x64.dmg'),
      windows_x64: publicUrl(
        baseUrl,
        prefix,
        'ArtForgeStudio_windows_x64_setup.exe',
      ),
      windows_x64_portable: publicUrl(
        baseUrl,
        prefix,
        'ArtForgeStudio_windows_x64_portable.zip',
      ),
    },
  };

  fs.mkdirSync(path.dirname(output), { recursive: true });
  fs.writeFileSync(output, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8');
  console.log(`Update manifest: ${output}`);
}

try {
  main();
} catch (error) {
  console.error(error.message);
  process.exit(1);
}
