// The page's shared state, in one place.
//
// These are plain objects, not a store: every module imports the same object,
// so a change made anywhere is seen everywhere at once, exactly as the single
// file did before. `hooks` holds the few calls that would otherwise make a
// circle of imports; it is filled in when the module that owns the call loads.

/** What the computer (or Spotify alone) says is playing, and what is shown over it. */
export const player = { state: null, online: false, direct: false, at: 0, held: new Map(), lastAct: 0, sheet: null, mini: null };

/** This phone's own player: its audio element, queue and place. */
export const here = {
  audio: null, list: [], index: -1, active: false, trouble: null, shuffle: false, repeat: 'off',
  seq: 0, tries: 0, place: 0, likedUri: null, liked: false, pausedByUser: false, playingUri: null, pending: false,
};

/** The computer's event stream, while this page is in front. */
export const events = { open: false, stop: null };
/** Answers being waited for, woken the moment the computer has put them up. */
export const replyWaiters = new Map();
export const readyReplies = new Set();

/** What the phone last told the computer it plays. */
export const told = { at: 0, timer: null, key: '' };

/** The computer looks in every so often while nobody is watching, so give it that long. */
export const WAKE_MS = 18000;
export const openedAt = Date.now();

/**
 * Filled in by the module that owns each call, so modules that need it do not
 * import upwards into a cycle. Reading one before it is set is the page asking
 * too early, never something a listener can reach.
 */
export const hooks = {
  /** This phone's player actions (`HERE_ACTS`). */
  hereActs: null,
  /** Play a list from a row tap (`playFrom`). */
  playFrom: null,
  /** What the player currently shows, over the computer's state (`shown`). */
  shown: null,
  /** With the computer off, read Spotify's own player (`pollSpotify`). */
  pollSpotify: null,
  /** Draw the current view; `true` reads it again. */
  render: null,
  /** Draw the banner about the computer. */
  renderBanner: null,
  /** Forget the kept tab views and draw again. */
  reloadViews: null,
  /** Remember where a view was scrolled to. */
  keepPlace: null,
  /** A shared song changed the group's feed. */
  forgetGroup: null,
  /** The device was removed: show the signed-out door. */
  signedOut: null,
  /** Start the app again from the top. */
  restart: null,
};
