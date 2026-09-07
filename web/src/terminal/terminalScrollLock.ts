export type TerminalScrollLock = {
  handleFocus: () => void;
  handleBlur: () => void;
  isFollowing: () => boolean;
  beginUserScroll: () => void;
  follow: (scrollToBottom: () => void) => void;
  handleInput: (scrollToBottom: () => void) => void;
  pinAfterResize: (scrollToBottom: () => void) => void;
  pinAfterOutput: (scrollToBottom: () => void) => void;
  handleScroll: (
    viewportY: number,
    baseY: number,
    scrollToBottom: () => void,
  ) => boolean;
};

export function createTerminalScrollLock(enabled: boolean): TerminalScrollLock {
  let followingLiveOutput = enabled;

  const pinIfFollowing = (scrollToBottom: () => void) => {
    if (enabled && followingLiveOutput) scrollToBottom();
  };

  const follow = (scrollToBottom: () => void) => {
    if (enabled) followingLiveOutput = true;
    scrollToBottom();
  };

  return {
    // Keyboard focus is independent from terminal scroll intent. In particular,
    // blur happens before iOS finishes its keyboard-close resize/reflow.
    handleFocus() {},
    handleBlur() {},
    isFollowing: () => !enabled || followingLiveOutput,
    beginUserScroll() {
      if (enabled) followingLiveOutput = false;
    },
    follow,
    handleInput: follow,
    pinAfterResize: pinIfFollowing,
    pinAfterOutput: pinIfFollowing,
    handleScroll(viewportY, baseY, scrollToBottom) {
      if (!enabled) return false;
      if (viewportY >= baseY) {
        followingLiveOutput = true;
        return false;
      }
      if (!followingLiveOutput) return false;
      scrollToBottom();
      return true;
    },
  };
}
