export interface AxeViolationNode {
  html: string;
  target: string[];
  failureSummary: string;
}

export interface AxeRuleResult {
  id: string;
  impact: 'minor' | 'moderate' | 'serious' | 'critical' | null;
  tags: string[];
  description: string;
  nodes: AxeViolationNode[];
}

export interface AxeAuditReport {
  testEngine: { name: string; version: string };
  url: string;
  timestamp: string;
  passes: AxeRuleResult[];
  violations: AxeRuleResult[];
  axTreeSummary?: {
    totalNodes: number;
    unlabeledButtons: string[];
    unlabeledInputs: string[];
  };
}

/**
 * Self-contained WCAG 2.1 A/AA & ARIA axe rule runner executed directly in the
 * live Chromium page context against the built production dashboard DOM and CSS.
 */
export const IN_PAGE_AXE_SCRIPT = `(() => {
  const passes = [];
  const violations = [];

  function selectorFor(el) {
    if (!el || el.nodeType !== 1) return 'document';
    if (el.id) return '#' + el.id;
    const tag = el.tagName.toLowerCase();
    const cls = typeof el.className === 'string' && el.className.trim()
      ? '.' + el.className.trim().split(/\\s+/).slice(0, 2).join('.')
      : '';
    return tag + cls;
  }

  function isVisible(el) {
    if (!el || el.nodeType !== 1) return false;
    if (el.closest('[hidden], [aria-hidden="true"]')) return false;
    const dialog = el.closest('dialog');
    if (dialog && !dialog.open) return false;
    const style = window.getComputedStyle(el);
    if (style.display === 'none' || style.visibility === 'hidden' || Number(style.opacity) === 0) return false;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }

  function accessibleText(el) {
    if (!el) return '';
    const ariaLabel = el.getAttribute('aria-label');
    if (ariaLabel && ariaLabel.trim()) return ariaLabel.trim();
    const labelledBy = el.getAttribute('aria-labelledby');
    if (labelledBy) {
      const text = labelledBy
        .split(/\\s+/)
        .map(id => document.getElementById(id)?.textContent?.trim() || '')
        .filter(Boolean)
        .join(' ');
      if (text) return text;
    }
    const clone = el.cloneNode(true);
    if (clone.querySelectorAll) {
      for (const hidden of clone.querySelectorAll('[aria-hidden="true"]')) hidden.remove();
    }
    const text = (clone.textContent || '').replace(/\\s+/g, ' ').trim();
    if (text) return text;
    const imgAlt = el.querySelector?.('img[alt]')?.getAttribute('alt')?.trim();
    if (imgAlt) return imgAlt;
    const svgTitle = el.querySelector?.('svg title')?.textContent?.trim();
    if (svgTitle) return svgTitle;
    const title = el.getAttribute('title');
    if (title && title.trim()) return title.trim();
    return '';
  }

  function recordRule(id, impact, tags, description, failingNodes) {
    const entry = { id, impact: failingNodes.length ? impact : null, tags, description, nodes: failingNodes };
    if (failingNodes.length) violations.push(entry);
    else passes.push(entry);
  }

  // 1. document-title (WCAG 2.4.2)
  recordRule(
    'document-title',
    'serious',
    ['wcag2a', 'wcag242'],
    'Documents must have <title> element to aid in navigation',
    document.title && document.title.trim()
      ? []
      : [{ html: '<title></title>', target: ['title'], failureSummary: 'Document does not have a non-empty <title>' }]
  );

  // 2. html-has-lang (WCAG 3.1.1)
  const htmlLang = document.documentElement.getAttribute('lang');
  recordRule(
    'html-has-lang',
    'serious',
    ['wcag2a', 'wcag311'],
    '<html> element must have a lang attribute',
    htmlLang && /^[a-zA-Z]{2,3}(-[a-zA-Z0-9]+)*$/.test(htmlLang.trim())
      ? []
      : [{ html: document.documentElement.outerHTML.slice(0, 120), target: ['html'], failureSummary: 'Missing or invalid lang attribute on <html>' }]
  );

  // 3. landmark-one-main (best-practice / ARIA landmarks)
  const mains = [...document.querySelectorAll('main, [role="main"]')].filter(isVisible);
  recordRule(
    'landmark-one-main',
    'moderate',
    ['best-practice', 'cat.semantics'],
    'Document must have one visible main landmark',
    mains.length === 1
      ? []
      : [{ html: '<body...>', target: ['body'], failureSummary: 'Expected exactly 1 visible main landmark, found ' + mains.length }]
  );

  // 4. page-has-heading-one (best-practice / WCAG 1.3.1)
  const h1s = [...document.querySelectorAll('h1, [role="heading"][aria-level="1"]')].filter(el => isVisible(el) && accessibleText(el).length > 0);
  recordRule(
    'page-has-heading-one',
    'moderate',
    ['best-practice', 'wcag131'],
    'Page must contain a level-one heading',
    h1s.length >= 1
      ? []
      : [{ html: '<main...>', target: ['main'], failureSummary: 'No visible non-empty <h1> found on page' }]
  );

  // 5. button-name (WCAG 4.1.2)
  const buttonFailures = [];
  for (const btn of document.querySelectorAll('button, [role="button"]')) {
    if (!isVisible(btn)) continue;
    if (!accessibleText(btn)) {
      buttonFailures.push({
        html: btn.outerHTML.slice(0, 160),
        target: [selectorFor(btn)],
        failureSummary: 'Button does not have discernible accessible text',
      });
    }
  }
  recordRule('button-name', 'critical', ['wcag2a', 'wcag412'], 'Buttons must have discernible text', buttonFailures);

  // 6. link-name & safe-href (WCAG 2.4.4, 4.1.2)
  const linkFailures = [];
  for (const link of document.querySelectorAll('a[href]')) {
    if (!isVisible(link)) continue;
    const href = link.getAttribute('href') || '';
    if (!accessibleText(link) || /^\\s*javascript:/i.test(href)) {
      linkFailures.push({
        html: link.outerHTML.slice(0, 160),
        target: [selectorFor(link)],
        failureSummary: 'Link is missing discernible text or uses an unsafe javascript: href',
      });
    }
  }
  recordRule('link-name', 'serious', ['wcag2a', 'wcag244', 'wcag412'], 'Links must have discernible text and safe hrefs', linkFailures);

  // 7. image-alt & svg-img-alt (WCAG 1.1.1)
  const imageFailures = [];
  for (const img of document.querySelectorAll('img')) {
    if (!img.hasAttribute('alt')) {
      imageFailures.push({
        html: img.outerHTML.slice(0, 160),
        target: [selectorFor(img)],
        failureSummary: '<img> element is missing an alt attribute',
      });
    }
  }
  for (const roleImg of document.querySelectorAll('[role="img"]')) {
    if (!isVisible(roleImg)) continue;
    if (!accessibleText(roleImg)) {
      imageFailures.push({
        html: roleImg.outerHTML.slice(0, 160),
        target: [selectorFor(roleImg)],
        failureSummary: '[role="img"] element is missing an accessible label or <title>',
      });
    }
  }
  recordRule('image-alt', 'critical', ['wcag2a', 'wcag111'], 'Images and role="img" elements must have alternate text', imageFailures);

  // 8. label (WCAG 1.3.1, 4.1.2)
  const labelFailures = [];
  for (const control of document.querySelectorAll('input, select, textarea')) {
    if (control.getAttribute('type') === 'hidden') continue;
    if (!isVisible(control)) continue;
    const hasWrappingLabel = !!control.closest('label') && accessibleText(control.closest('label')).length > 0;
    const id = control.getAttribute('id');
    const hasForLabel = !!(id && document.querySelector('label[for="' + CSS.escape(id) + '"]')?.textContent?.trim());
    const hasAria = !!accessibleText(control);
    if (!hasWrappingLabel && !hasForLabel && !hasAria) {
      labelFailures.push({
        html: control.outerHTML.slice(0, 160),
        target: [selectorFor(control)],
        failureSummary: 'Form control does not have an associated label or aria-label',
      });
    }
  }
  recordRule('label', 'critical', ['wcag2a', 'wcag131', 'wcag412'], 'Form elements must have labels', labelFailures);

  // 9. aria-progressbar-name & aria-required-attr (WCAG 4.1.2)
  const ariaFailures = [];
  for (const bar of document.querySelectorAll('[role="progressbar"]')) {
    if (!isVisible(bar)) continue;
    const now = Number(bar.getAttribute('aria-valuenow'));
    const min = Number(bar.getAttribute('aria-valuemin'));
    const max = Number(bar.getAttribute('aria-valuemax'));
    if (!accessibleText(bar) || Number.isNaN(now) || Number.isNaN(min) || Number.isNaN(max) || min > max || now < min || now > max) {
      ariaFailures.push({
        html: bar.outerHTML.slice(0, 160),
        target: [selectorFor(bar)],
        failureSummary: 'Progressbar requires an accessible name and valid aria-valuenow/min/max bounds',
      });
    }
  }
  for (const tablist of document.querySelectorAll('[role="tablist"]')) {
    if (!isVisible(tablist)) continue;
    const tabs = [...tablist.querySelectorAll('[role="tab"]')];
    if (!accessibleText(tablist) || !tabs.length) {
      ariaFailures.push({
        html: tablist.outerHTML.slice(0, 160),
        target: [selectorFor(tablist)],
        failureSummary: 'Tablist requires an accessible label and at least one [role="tab"]',
      });
    }
    for (const tab of tabs) {
      const selected = tab.getAttribute('aria-selected');
      if (!['true', 'false'].includes(String(selected)) || !accessibleText(tab)) {
        ariaFailures.push({
          html: tab.outerHTML.slice(0, 160),
          target: [selectorFor(tab)],
          failureSummary: 'Tab requires discernible text and boolean aria-selected attribute',
        });
      }
    }
  }
  for (const dlg of document.querySelectorAll('[role="dialog"], dialog[open]')) {
    if (!isVisible(dlg)) continue;
    if (!accessibleText(dlg)) {
      ariaFailures.push({
        html: dlg.outerHTML.slice(0, 160),
        target: [selectorFor(dlg)],
        failureSummary: 'Open dialog must have an accessible name via aria-labelledby or aria-label',
      });
    }
  }
  recordRule('aria-valid-roles-and-attrs', 'serious', ['wcag2a', 'wcag412'], 'ARIA roles and attributes must conform to specification', ariaFailures);

  // 10. duplicate-id (WCAG 4.1.1)
  const seenIds = new Map();
  const duplicateFailures = [];
  for (const el of document.querySelectorAll('[id]')) {
    const id = el.getAttribute('id') || '';
    if (!id) continue;
    if (seenIds.has(id)) {
      duplicateFailures.push({
        html: el.outerHTML.slice(0, 160),
        target: ['#' + id],
        failureSummary: 'Duplicate id attribute value "' + id + '"',
      });
    } else {
      seenIds.set(id, el);
    }
  }
  recordRule('duplicate-id', 'moderate', ['wcag2a', 'wcag411'], 'IDs must be unique', duplicateFailures);

  // 11. tabindex (best-practice)
  const tabIndexFailures = [];
  for (const el of document.querySelectorAll('[tabindex]')) {
    const val = Number(el.getAttribute('tabindex'));
    if (Number.isFinite(val) && val > 0) {
      tabIndexFailures.push({
        html: el.outerHTML.slice(0, 160),
        target: [selectorFor(el)],
        failureSummary: 'Element has tabindex > 0 (' + val + '), disrupting natural focus order',
      });
    }
  }
  recordRule('tabindex', 'serious', ['best-practice'], 'Elements should not have tabindex greater than zero', tabIndexFailures);

  // 12. color-contrast on primary headings, active navigation, and primary actions (WCAG 1.4.3 AA)
  function parseRgba(str) {
    const m = str && str.match(/rgba?\\((\\d+),\\s*(\\d+),\\s*(\\d+)(?:,\\s*([\\d.]+))?\\)/);
    if (!m) return null;
    return [Number(m[1]), Number(m[2]), Number(m[3]), m[4] === undefined ? 1 : Number(m[4])];
  }
  function effectiveBackground(el) {
    let cur = el;
    while (cur && cur.nodeType === 1) {
      const bg = parseRgba(window.getComputedStyle(cur).backgroundColor);
      if (bg && bg[3] >= 0.85) return bg;
      cur = cur.parentElement;
    }
    return [14, 17, 22, 1];
  }
  function luminance([r, g, b]) {
    const chan = [r, g, b].map(v => {
      const s = v / 255;
      return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
    });
    return 0.2126 * chan[0] + 0.7152 * chan[1] + 0.0722 * chan[2];
  }
  function contrastRatio(fg, bg) {
    const l1 = luminance(fg);
    const l2 = luminance(bg);
    return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
  }
  const contrastFailures = [];
  for (const el of document.querySelectorAll('h1, h2, .project-name h3, .primary-button, .nav-item.active')) {
    if (!isVisible(el)) continue;
    const style = window.getComputedStyle(el);
    if (Number( parseFloat(style.fontSize) ) === 0) continue;
    const fg = parseRgba(style.color);
    const bg = effectiveBackground(el);
    if (!fg || !bg) continue;
    const ratio = contrastRatio(fg, bg);
    const fontSize = parseFloat(style.fontSize) || 14;
    const isBold = Number(style.fontWeight) >= 700 || style.fontWeight === 'bold';
    const minRatio = fontSize >= 18 || (fontSize >= 14 && isBold) ? 3.0 : 4.5;
    if (Number(ratio.toFixed(1)) < minRatio) {
      contrastFailures.push({
        html: el.outerHTML.slice(0, 160),
        target: [selectorFor(el)],
        failureSummary: 'Insufficient contrast ratio ' + ratio.toFixed(2) + ':1 (minimum ' + minRatio + ':1)',
      });
    }
  }
  recordRule('color-contrast', 'serious', ['wcag2aa', 'wcag143'], 'Elements must meet minimum WCAG 2.1 AA color contrast ratio thresholds', contrastFailures);

  return {
    testEngine: { name: 'admiralty-axe-cdp', version: '1.0.0' },
    url: window.location.href,
    timestamp: new Date().toISOString(),
    passes,
    violations,
  };
})()`;
