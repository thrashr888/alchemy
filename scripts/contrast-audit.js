(() => {
  // WCAG contrast audit of the current view under the CURRENT theme.
  // Returns worst offenders: visible text whose effective contrast < 4.5
  // (3.0 for >=19px bold/24px), plus any text whose background chain ends
  // transparent (undefined contrast = the void-background bug class).
  const parse = (s) => {
    const m = s.match(/rgba?\(([\d.]+)[, ]+([\d.]+)[, ]+([\d.]+)(?:[,/ ]+([\d.]+))?\)/);
    if (!m) return null;
    return { r: +m[1], g: +m[2], b: +m[3], a: m[4] === undefined ? 1 : +m[4] };
  };
  const over = (top, bot) => {
    // composite top (maybe translucent) over bot (opaque)
    const a = top.a + bot.a * (1 - top.a);
    if (a === 0) return { r: 0, g: 0, b: 0, a: 0 };
    return {
      r: (top.r * top.a + bot.r * bot.a * (1 - top.a)) / a,
      g: (top.g * top.a + bot.g * bot.a * (1 - top.a)) / a,
      b: (top.b * top.a + bot.b * bot.a * (1 - top.a)) / a,
      a,
    };
  };
  const lum = (c) => {
    const f = (v) => {
      v /= 255;
      return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
    };
    return 0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b);
  };
  const ratio = (a, b) => {
    const l1 = lum(a), l2 = lum(b);
    return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
  };
  // Effective background: walk up compositing translucent layers until an
  // opaque one; report void:true if the chain ends still-translucent.
  const effBg = (el) => {
    let acc = { r: 0, g: 0, b: 0, a: 0 };
    let node = el;
    while (node && node !== document.documentElement) {
      const bg = parse(getComputedStyle(node).backgroundColor);
      if (bg && bg.a > 0) {
        acc = acc.a === 0 ? bg : over(acc, bg);
        if (acc.a >= 0.99) return { color: acc, voidBg: false };
      }
      node = node.parentElement;
    }
    const bodyBg = parse(getComputedStyle(document.body).backgroundColor);
    if (bodyBg && bodyBg.a > 0) {
      acc = acc.a === 0 ? bodyBg : over(acc, bodyBg);
    }
    return { color: acc, voidBg: acc.a < 0.99 };
  };
  const offenders = [];
  const els = document.querySelectorAll("body *");
  let checked = 0;
  for (const el of els) {
    if (checked > 1200) break;
    // direct text only
    let text = "";
    for (const n of el.childNodes)
      if (n.nodeType === 3) text += n.textContent;
    text = text.trim();
    if (!text) continue;
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || cs.display === "none") continue;
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) continue;
    if (+cs.opacity === 0) continue;
    checked++;
    const fg = parse(cs.color);
    if (!fg) continue;
    const { color: bg, voidBg } = effBg(el);
    const size = parseFloat(cs.fontSize);
    const bold = parseInt(cs.fontWeight, 10) >= 600;
    const large = size >= 24 || (size >= 19 && bold);
    const need = large ? 3 : 4.5;
    // fg may be translucent — composite over bg first
    const fgEff = fg.a < 1 && bg.a > 0 ? over(fg, bg) : fg;
    const c = bg.a > 0 ? ratio(fgEff, bg) : 0;
    if (voidBg || c < need) {
      offenders.push({
        text: text.slice(0, 32),
        cls: (el.getAttribute("class") || "").split(" ").slice(0, 3).join(" "),
        ratio: Math.round(c * 100) / 100,
        need,
        voidBg,
        size: Math.round(size),
      });
    }
  }
  offenders.sort((a, b) => a.ratio - b.ratio);
  return JSON.stringify({
    theme: document.documentElement.dataset.theme,
    checked,
    offenders: offenders.slice(0, 8),
  });
})()
