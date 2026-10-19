/**
 * navigation/index.ts — public surface of the navigation layer.
 *
 * The screen agents import the nav hook + the `navigate` API from here.
 */

export {
  useNavStore,
  navigate,
  openClaude,
  currentScreen,
  deriveScreen,
  nextScreen,
  bindConnectionToNavigation,
  type Screen,
  type NavState,
} from './navStore';
