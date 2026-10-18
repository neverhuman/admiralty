import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const PINNED_AXE_CORE_VERSION = '4.14.0';
export const PINNED_AXE_CORE_SHA256 =
  '20c09fe157a8a34a30e241aaa1fcdade657734f08ab379ecfbeb7d45cc46e878';
export const WCAG21_AA_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'] as const;

export interface AxeNodeResult {
  html: string;
  target: string[];
  failureSummary?: string;
  impact?: 'minor' | 'moderate' | 'serious' | 'critical' | null;
}

export interface AxeRuleResult {
  id: string;
  impact?: 'minor' | 'moderate' | 'serious' | 'critical' | null;
  tags: string[];
  description: string;
  help: string;
  helpUrl: string;
  nodes: AxeNodeResult[];
}

export interface AxeAuditReport {
  url: string;
  timestamp: string;
  testEngine: {
    name: string;
    version: string;
  };
  sourceSha256: string;
  violations: AxeRuleResult[];
  incomplete: AxeRuleResult[];
  passesCount: number;
  inapplicableCount: number;
}

export interface CustomAxTreeSmokeSummary {
  totalNodes: number;
  unlabeledButtons: string[];
  unlabeledInputs: string[];
}

export interface ResolvedAxeCoreBundle {
  axeMinJsPath: string;
  version: string;
  sha256: string;
  source: string;
}

let cachedBundle: ResolvedAxeCoreBundle | null = null;

export function loadPinnedAxeCoreBundle(): ResolvedAxeCoreBundle {
  if (cachedBundle) return cachedBundle;

  const e2eDir = dirname(fileURLToPath(import.meta.url));
  const workspaceRoot = resolve(e2eDir, '..', '..', '..');
  const req = createRequire(import.meta.url);

  const candidates: string[] = [];
  try {
    candidates.push(req.resolve('axe-core/axe.min.js'));
  } catch {
    // Fallback to deterministic local workspace / worktree paths if node_modules link is not yet present
  }
  candidates.push(
    join(workspaceRoot, 'node_modules', 'axe-core', 'axe.min.js'),
    join(workspaceRoot, 'node_modules', '.pnpm', `axe-core@${PINNED_AXE_CORE_VERSION}`, 'node_modules', 'axe-core', 'axe.min.js'),
    `/home/ubuntu/adm_surge/2/node_modules/.pnpm/axe-core@${PINNED_AXE_CORE_VERSION}/node_modules/axe-core/axe.min.js`,
  );

  const axeMinJsPath = candidates.find((candidate) => existsSync(candidate));
  if (!axeMinJsPath) {
    throw new Error(
      `Pinned axe-core@${PINNED_AXE_CORE_VERSION} bundle not found via createRequire(import.meta.url).resolve('axe-core/axe.min.js'). ` +
        `Ensure axe-core@${PINNED_AXE_CORE_VERSION} is installed in workspace node_modules.`,
    );
  }

  const pkgJsonPath = join(dirname(axeMinJsPath), 'package.json');
  if (!existsSync(pkgJsonPath)) {
    throw new Error(`Missing axe-core package.json alongside ${axeMinJsPath}`);
  }
  const pkg = JSON.parse(readFileSync(pkgJsonPath, 'utf8')) as { name?: string; version?: string };
  if (pkg.name !== 'axe-core' || pkg.version !== PINNED_AXE_CORE_VERSION) {
    throw new Error(
      `Unexpected axe-core package at ${pkgJsonPath}: expected axe-core@${PINNED_AXE_CORE_VERSION}, found ${pkg.name ?? 'unknown'}@${pkg.version ?? 'unknown'}`,
    );
  }

  const rawBuffer = readFileSync(axeMinJsPath);
  const sha256 = createHash('sha256').update(rawBuffer).digest('hex');
  if (sha256 !== PINNED_AXE_CORE_SHA256) {
    throw new Error(
      `Pinned axe-core integrity mismatch for ${axeMinJsPath}: expected sha256 ${PINNED_AXE_CORE_SHA256}, got ${sha256}`,
    );
  }

  cachedBundle = {
    axeMinJsPath,
    version: pkg.version,
    sha256,
    source: rawBuffer.toString('utf8'),
  };
  return cachedBundle;
}

export function buildAxeRunExpression(
  tags: readonly string[] = WCAG21_AA_TAGS,
  expectedSha256: string = PINNED_AXE_CORE_SHA256,
): string {
  const tagsJson = JSON.stringify(tags);
  const shaJson = JSON.stringify(expectedSha256);
  return `(async () => {
    if (typeof window.axe?.run !== 'function') {
      throw new Error('axe-core is not loaded on window.axe');
    }
    const raw = await window.axe.run(document, {
      runOnly: {
        type: 'tag',
        values: ${tagsJson},
      },
      resultTypes: ['violations', 'incomplete', 'passes', 'inapplicable'],
    });
    const mapRules = (items) => (Array.isArray(items) ? items : []).map((item) => ({
      id: String(item.id),
      impact: item.impact ?? null,
      tags: Array.isArray(item.tags) ? item.tags.map(String) : [],
      description: String(item.description ?? ''),
      help: String(item.help ?? ''),
      helpUrl: String(item.helpUrl ?? ''),
      nodes: (Array.isArray(item.nodes) ? item.nodes : []).map((n) => ({
        html: String(n.html ?? ''),
        target: Array.isArray(n.target) ? n.target.map((t) => (Array.isArray(t) ? t.join(' ') : String(t))) : [],
        failureSummary: typeof n.failureSummary === 'string' ? n.failureSummary : undefined,
        impact: n.impact ?? null,
      })),
    }));
    return {
      url: String(raw.url || window.location.href),
      timestamp: String(raw.timestamp || new Date().toISOString()),
      testEngine: {
        name: String(raw.testEngine?.name ?? ''),
        version: String(raw.testEngine?.version ?? ''),
      },
      sourceSha256: ${shaJson},
      violations: mapRules(raw.violations),
      incomplete: mapRules(raw.incomplete),
      passesCount: Array.isArray(raw.passes) ? raw.passes.length : 0,
      inapplicableCount: Array.isArray(raw.inapplicable) ? raw.inapplicable.length : 0,
    };
  })()`;
}
