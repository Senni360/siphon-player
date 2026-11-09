// The two errors the page itself adds meaning to.

/** The service no longer accepts this device's secret. */
export class SignedOut extends Error {}

/** An answer from the computer, rather than a failure on the way to it. */
export const answered = message => Object.assign(new Error(message), { answered: true });
