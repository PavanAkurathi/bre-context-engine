#!/usr/bin/env node

import { existsSync } from "node:fs";
import fs from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const VITE_CONFIG_CANDIDATES = [
  "vite.config.ts",
  "vite.config.mts",
  "vite.config.cts",
  "vite.config.js",
  "vite.config.mjs",
  "vite.config.cjs",
];

const args = parseArgs(process.argv.slice(2));

if (args.help) {
  console.log(`Usage: node tools/hydrate-js-aliases.mjs [repo-root] [options]

Options:
  --bre-json <path>  bre.json path, relative to repo root unless absolute
  --stdout           print { "js_aliases": ... } instead of updating bre.json
  --help             show this help
`);
  process.exit(0);
}

const repoRoot = await fs.realpath(path.resolve(args.repoRoot ?? process.cwd()));
const breJsonPath = resolveRepoPath(repoRoot, args.breJson ?? "bre.json");

const aliases = {};
mergeAliasMaps(aliases, await aliasesFromTsConfig(repoRoot));
mergeAliasMaps(aliases, await aliasesFromVite(repoRoot));

if (args.stdout) {
  console.log(JSON.stringify({ js_aliases: aliases }, null, 2));
} else {
  if (!existsSync(breJsonPath)) {
    fail(`cannot update missing bre.json: ${breJsonPath}`);
  }
  const breConfig = await readJsonFile(breJsonPath);
  breConfig.js_aliases = breConfig.js_aliases ?? {};
  mergeAliasMaps(breConfig.js_aliases, aliases);
  await fs.writeFile(breJsonPath, `${JSON.stringify(breConfig, null, 2)}\n`, "utf8");
  console.error(`hydrated ${Object.keys(aliases).length} JS alias entries into ${breJsonPath}`);
}

function parseArgs(argv) {
  const parsed = {
    repoRoot: undefined,
    breJson: undefined,
    stdout: false,
    help: false,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") {
      parsed.help = true;
    } else if (arg === "--stdout") {
      parsed.stdout = true;
    } else if (arg === "--bre-json") {
      const value = argv[index + 1];
      if (!value) {
        fail("--bre-json requires a path");
      }
      parsed.breJson = value;
      index += 1;
    } else if (arg.startsWith("--")) {
      fail(`unknown option: ${arg}`);
    } else if (parsed.repoRoot === undefined) {
      parsed.repoRoot = arg;
    } else {
      fail(`unexpected argument: ${arg}`);
    }
  }

  return parsed;
}

function resolveRepoPath(repoRoot, maybeRelative) {
  return path.isAbsolute(maybeRelative) ? maybeRelative : path.join(repoRoot, maybeRelative);
}

async function aliasesFromTsConfig(repoRoot) {
  const aliases = {};
  for (const fileName of ["tsconfig.json", "jsconfig.json"]) {
    const configPath = path.join(repoRoot, fileName);
    if (!existsSync(configPath)) {
      continue;
    }

    mergeAliasMaps(aliases, await aliasesFromTsConfigFile(repoRoot, configPath, new Set()));
  }
  return aliases;
}

async function aliasesFromTsConfigFile(repoRoot, configPath, visited) {
  const realConfigPath = await fs.realpath(configPath).catch(() => configPath);
  if (visited.has(realConfigPath)) {
    return {};
  }
  visited.add(realConfigPath);

  const aliases = {};
  const config = await readJsoncFile(configPath);
  const configDir = path.dirname(configPath);

  if (typeof config.extends === "string") {
    const extendedPath = resolveTsConfigExtends(configDir, config.extends);
    if (extendedPath && existsSync(extendedPath)) {
      mergeAliasMaps(aliases, await aliasesFromTsConfigFile(repoRoot, extendedPath, visited));
    }
  }

  const compilerOptions = config.compilerOptions ?? {};
  const paths = compilerOptions.paths ?? {};
  const baseUrl = compilerOptions.baseUrl ?? ".";

  for (const [alias, targets] of Object.entries(paths)) {
    const targetList = Array.isArray(targets) ? targets : [targets];
    for (const target of targetList) {
      if (typeof target !== "string") {
        continue;
      }
      addAlias(aliases, alias, joinAliasTarget(configDir, baseUrl, target), repoRoot);
    }
  }

  return aliases;
}

function resolveTsConfigExtends(configDir, extendsValue) {
  if (!extendsValue.startsWith(".") && !path.isAbsolute(extendsValue)) {
    return undefined;
  }

  const basePath = path.isAbsolute(extendsValue)
    ? extendsValue
    : path.resolve(configDir, extendsValue);
  if (existsSync(basePath)) {
    return basePath;
  }
  if (existsSync(`${basePath}.json`)) {
    return `${basePath}.json`;
  }
  return undefined;
}

async function aliasesFromVite(repoRoot) {
  const configPath = VITE_CONFIG_CANDIDATES.map((fileName) => path.join(repoRoot, fileName)).find(
    existsSync,
  );
  if (!configPath) {
    return {};
  }

  const viteConfig = await loadViteConfig(repoRoot, configPath);
  return aliasesFromViteConfig(viteConfig, repoRoot);
}

async function loadViteConfig(repoRoot, configPath) {
  const requireFromRoot = createRequire(path.join(repoRoot, "package.json"));

  try {
    const vitePath = requireFromRoot.resolve("vite");
    const vite = await import(pathToFileURL(vitePath).href);
    if (typeof vite.loadConfigFromFile === "function") {
      const loaded = await withCwd(repoRoot, () =>
        vite.loadConfigFromFile(
          {
            command: "serve",
            mode: process.env.NODE_ENV ?? "development",
            isSsrBuild: false,
            isPreview: false,
          },
          configPath,
          repoRoot,
          "silent",
        ),
      );
      if (loaded?.config) {
        return loaded.config;
      }
    }
  } catch {
    // Vite is optional for this adapter; tsconfig/jsconfig aliases are still useful.
  }

  if (configPath.endsWith(".cjs")) {
    const mod = requireFromRoot(configPath);
    return await normalizeDefaultExport(mod);
  }

  if (configPath.endsWith(".js") || configPath.endsWith(".mjs")) {
    const mod = await import(pathToFileURL(configPath).href);
    return await normalizeDefaultExport(mod);
  }

  return {};
}

async function withCwd(cwd, callback) {
  const previousCwd = process.cwd();
  process.chdir(cwd);
  try {
    return await callback();
  } finally {
    process.chdir(previousCwd);
  }
}

async function normalizeDefaultExport(mod) {
  const value = mod?.default ?? mod;
  return typeof value === "function"
    ? await value({ command: "serve", mode: "development" })
    : value;
}

function aliasesFromViteConfig(config, repoRoot) {
  const aliases = {};
  const aliasConfig = config?.resolve?.alias;
  if (!aliasConfig) {
    return aliases;
  }

  if (Array.isArray(aliasConfig)) {
    for (const entry of aliasConfig) {
      addAlias(aliases, entry?.find, entry?.replacement, repoRoot);
    }
    return aliases;
  }

  if (typeof aliasConfig === "object") {
    for (const [find, replacement] of Object.entries(aliasConfig)) {
      addAlias(aliases, find, replacement, repoRoot);
    }
  }

  return aliases;
}

function addAlias(aliases, find, replacement, repoRoot) {
  if (typeof find !== "string" || typeof replacement !== "string") {
    return;
  }

  const target = normalizeAliasTarget(repoRoot, replacement);
  if (!target) {
    return;
  }

  aliases[find] = aliases[find] ?? [];
  if (!aliases[find].includes(target)) {
    aliases[find].push(target);
  }
}

function joinAliasTarget(configDir, baseUrl, target) {
  if (path.isAbsolute(target)) {
    return target;
  }
  return path.join(configDir, baseUrl, target);
}

function normalizeAliasTarget(repoRoot, target) {
  let normalized = target;
  if (normalized.startsWith("file://")) {
    normalized = fileURLToPath(normalized);
  }

  if (path.isAbsolute(normalized)) {
    normalized = path.relative(repoRoot, normalized);
  }

  normalized = normalized.replaceAll(path.sep, "/");
  normalized = normalized.replace(/^\.\//, "");
  return normalized || ".";
}

function mergeAliasMaps(target, source) {
  for (const [alias, sourceTargets] of Object.entries(source ?? {})) {
    target[alias] = target[alias] ?? [];
    const targetList = Array.isArray(target[alias]) ? target[alias] : [target[alias]];
    target[alias] = targetList.filter((value) => typeof value === "string");
    const sourceList = Array.isArray(sourceTargets) ? sourceTargets : [sourceTargets];
    for (const sourceTarget of sourceList) {
      if (typeof sourceTarget === "string" && !target[alias].includes(sourceTarget)) {
        target[alias].push(sourceTarget);
      }
    }
  }
}

async function readJsonFile(filePath) {
  const content = await fs.readFile(filePath, "utf8");
  return JSON.parse(content);
}

async function readJsoncFile(filePath) {
  const content = await fs.readFile(filePath, "utf8");
  return JSON.parse(removeTrailingCommas(stripJsonComments(content)));
}

function stripJsonComments(input) {
  let output = "";
  let inString = false;
  let quote = "";
  let escaped = false;
  let inLineComment = false;
  let inBlockComment = false;

  for (let index = 0; index < input.length; index += 1) {
    const char = input[index];
    const next = input[index + 1];

    if (inLineComment) {
      if (char === "\n") {
        inLineComment = false;
        output += char;
      }
      continue;
    }

    if (inBlockComment) {
      if (char === "\n") {
        output += char;
      }
      if (char === "*" && next === "/") {
        inBlockComment = false;
        index += 1;
      }
      continue;
    }

    if (inString) {
      output += char;
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === quote) {
        inString = false;
      }
      continue;
    }

    if (char === '"' || char === "'") {
      inString = true;
      quote = char;
      output += char;
    } else if (char === "/" && next === "/") {
      inLineComment = true;
      index += 1;
    } else if (char === "/" && next === "*") {
      inBlockComment = true;
      index += 1;
    } else {
      output += char;
    }
  }

  return output;
}

function removeTrailingCommas(input) {
  let output = "";
  let inString = false;
  let quote = "";
  let escaped = false;

  for (let index = 0; index < input.length; index += 1) {
    const char = input[index];

    if (inString) {
      output += char;
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === quote) {
        inString = false;
      }
      continue;
    }

    if (char === '"' || char === "'") {
      inString = true;
      quote = char;
      output += char;
      continue;
    }

    if (char === ",") {
      let lookahead = index + 1;
      while (/\s/.test(input[lookahead] ?? "")) {
        lookahead += 1;
      }
      if (input[lookahead] === "}" || input[lookahead] === "]") {
        continue;
      }
    }

    output += char;
  }

  return output;
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
