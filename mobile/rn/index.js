/**
 * Okena mobile — React Native entry point.
 *
 * Registers the root component with the native host. The app name must match
 * `app.json`'s `name` and the value the generated Android/iOS host projects
 * pass to `ReactActivityDelegate` / `RCTRootView`.
 *
 * @format
 */

import {AppRegistry} from 'react-native';

import App from './src/App';
import {name as appName} from './app.json';

AppRegistry.registerComponent(appName, () => App);
