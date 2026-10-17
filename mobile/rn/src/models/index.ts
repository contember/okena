/**
 * models/index.ts — public surface of the data models.
 */

export {
  createSavedServer,
  savedServerDisplayName,
  savedServerEquals,
  withSavedServer,
  toJSON as savedServerToJSON,
  fromJSON as savedServerFromJSON,
  listFromJson as savedServersFromJson,
  listToJson as savedServersToJson,
  type SavedServer,
  type SavedServerJson,
} from './savedServer';

export {
  parseLayout,
  type LayoutNode,
  type LayoutNodeType,
  type TerminalNode,
  type SplitNode,
  type TabsNode,
} from './layoutNode';

export {
  buildProjectList,
  filterProjectList,
  flattenProjectList,
  manualProjectOrder,
  projectDisplayName,
  projectMatches,
  type ProjectListItem,
  type ProjectListRow,
  type ProjectNode,
} from './projectList';

export {
  ACTIVITY_TIER_LABELS,
  RUNNING_OUTPUT_WINDOW_MS,
  applySnapshot,
  buildActivityRows,
  computeProjectSignals,
  orderByActivity,
  projectSignals,
  signalsEqual,
  snapshotRows,
  switcherRows,
  type ActivityEntry,
  type ActivityRow,
  type ActivitySnapshot,
  type ActivityTier,
  type OrderedEntry,
  type ProjectSignals,
} from './activityOrder';

export {
  pickTerminal,
  terminalLabel,
  findTerminalPath,
  resolveRestore,
  type SelectionMemory,
  type RestoreOutcome,
} from './terminalSelection';
