import { describe, expect, test } from "bun:test";
import { createTerminalTakeover } from "../src/terminal/terminalTakeover";

describe("TerminalTakeover", () => {
  test("keeps the overlay until an accepted claim is settled", () => {
    const takeover = createTerminalTakeover(true);

    expect(takeover.requestResize(41, 30)).toEqual({
      cols: 41,
      rows: 30,
      claim: true,
    });
    expect(takeover.status()).toBe("claiming");

    takeover.handleAcknowledgement(41, 30, true);
    expect(takeover.status()).toBe("settling");

    takeover.settle();
    expect(takeover.status()).toBe("ready");
  });

  test("uses non-claiming resizes after authority is acknowledged", () => {
    const takeover = createTerminalTakeover(true);
    takeover.requestResize(41, 30);
    takeover.handleAcknowledgement(41, 30, true);
    takeover.settle();

    expect(takeover.requestResize(41, 18)).toEqual({
      cols: 41,
      rows: 18,
      claim: false,
    });
    expect(takeover.status()).toBe("resizing");
  });

  test("desktop authority blocks automatic resize until explicit takeover", () => {
    const takeover = createTerminalTakeover(true);
    takeover.handleAuthorityChange(true);

    expect(takeover.status()).toBe("blocked");
    expect(takeover.requestResize(41, 30)).toBeNull();
    expect(takeover.requestResize(41, 30, true)).toEqual({
      cols: 41,
      rows: 30,
      claim: true,
    });
    expect(takeover.status()).toBe("claiming");
  });

  test("a denied claim returns to the blocked overlay", () => {
    const takeover = createTerminalTakeover(true);
    takeover.requestResize(41, 30);

    takeover.handleAcknowledgement(84, 90, false);

    expect(takeover.status()).toBe("blocked");
  });
});
