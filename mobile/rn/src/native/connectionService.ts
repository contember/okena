import { PermissionsAndroid, Platform } from 'react-native';
import NativeConnectionService from '../specs/NativeConnectionService';

export interface ConnectionService {
  setActive(active: boolean): void;
  isRunning(): boolean;
}

let requestedNotifications = false;

export const connectionService: ConnectionService = {
  setActive(active) {
    if (Platform.OS !== 'android') return;
    NativeConnectionService?.setActive(active);
    if (active && Platform.Version >= 33 && !requestedNotifications) {
      requestedNotifications = true;
      void PermissionsAndroid.request(
        PermissionsAndroid.PERMISSIONS.POST_NOTIFICATIONS,
      ).catch((error: unknown) =>
        console.warn('Could not request connection notifications', error),
      );
    }
  },
  isRunning: () =>
    Platform.OS === 'android' &&
    (NativeConnectionService?.isRunning() ?? false),
};

export function onConnectionNotificationDisconnect(
  disconnect: () => void,
): () => void {
  const subscription = NativeConnectionService?.onDisconnect(disconnect);
  return () => subscription?.remove();
}
