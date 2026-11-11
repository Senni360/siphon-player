// Comparing what is typed or named, whatever its case, accents or punctuation.

/** Letters and digits only, without accents, so "M83." is "m83". */
export const plain = text => String(text ?? '').normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim();

/** Where in `texts` every typed word is found, in the order they came. */
export function matching(texts, typed) {
  const words = plain(typed).split(' ').filter(Boolean);
  const at = [];
  if (!words.length) return at;
  texts.forEach((text, index) => { if (words.every(word => text.includes(word))) at.push(index); });
  return at;
}
