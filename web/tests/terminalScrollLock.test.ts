import { describe, expect, test } from "bun:test";
import { createTerminalScrollLock } from "../src/terminal/terminalScrollLock";

function scrollCounter() {
  let count = 0;
  return {
    scroll: () => {
      count += 1;
    },
    count: () => count,
  };
}

describe("TerminalScrollLock", () => {
  test("keeps following live output across keyboard blur and close reflow", () => {
    const lock = createTerminalScrollLock(true);
    const bottom = scrollCounter();

    lock.handleFocus();
    lock.handleBlur();
    lock.pinAfterResize(bottom.scroll);
    const corrected = lock.handleScroll(12, 20, bottom.scroll);

    expect(corrected).toBe(true);
    expect(bottom.count()).toBe(2);
  });

  test("preserves manual scrollback across keyboard resize and live output", () => {
    const lock = createTerminalScrollLock(true);
    const bottom = scrollCounter();

    lock.beginUserScroll();
    lock.handleBlur();
    lock.pinAfterResize(bottom.scroll);
    lock.pinAfterOutput(bottom.scroll);
    const corrected = lock.handleScroll(12, 20, bottom.scroll);

    expect(corrected).toBe(false);
    expect(bottom.count()).toBe(0);
  });

  test("user input leaves manual scrollback and resumes following", () => {
    const lock = createTerminalScrollLock(true);
    const bottom = scrollCounter();

    lock.beginUserScroll();
    lock.handleInput(bottom.scroll);
    const corrected = lock.handleScroll(12, 20, bottom.scroll);

    expect(corrected).toBe(true);
    expect(bottom.count()).toBe(2);
  });

  test("reaching the bottom manually resumes following live output", () => {
    const lock = createTerminalScrollLock(true);
    const bottom = scrollCounter();

    lock.beginUserScroll();
    lock.handleScroll(20, 20, bottom.scroll);
    lock.pinAfterOutput(bottom.scroll);

    expect(bottom.count()).toBe(1);
  });

  test("explicit Bottom action resumes following", () => {
    const lock = createTerminalScrollLock(true);
    const bottom = scrollCounter();

    lock.beginUserScroll();
    lock.follow(bottom.scroll);
    lock.pinAfterResize(bottom.scroll);

    expect(bottom.count()).toBe(2);
  });
});
