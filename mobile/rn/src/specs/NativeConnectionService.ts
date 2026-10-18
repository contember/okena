import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export interface Spec extends TurboModule {
  setActive(active: boolean): void;
  isRunning(): boolean;
  readonly onDisconnect: CodegenTypes.EventEmitter<boolean>;
}

export default TurboModuleRegistry.get<Spec>('OkenaConnectionService');
