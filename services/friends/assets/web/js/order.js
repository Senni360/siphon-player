// A list in a new order, with no page and no player.

export function shuffled(list) {
  const out = [...list];
  for (let at = out.length - 1; at > 0; at--) {
    const other = Math.floor(Math.random() * (at + 1));
    [out[at], out[other]] = [out[other], out[at]];
  }
  return out;
}
