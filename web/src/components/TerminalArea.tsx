import type { ApiLayoutNode, ApiProject } from "../api/types";
import { TerminalPane } from "./TerminalPane";
import { SplitLayout } from "./SplitLayout";
import { TabLayout } from "./TabLayout";
import { KruhPane } from "./KruhPane/KruhPane";
import { useApp } from "../state/store";

export function TerminalArea({
  layout,
  project,
}: {
  layout: ApiLayoutNode;
  project: ApiProject;
}) {
  return (
    <div className="h-full">
      <LayoutRenderer node={layout} project={project} path={[]} />
    </div>
  );
}

export function LayoutRenderer({
  node,
  project,
  path,
}: {
  node: ApiLayoutNode;
  project: ApiProject;
  path: number[];
}) {
  switch (node.type) {
    case "terminal":
      return (
        <TerminalPane
          terminalId={node.terminal_id}
          name={node.terminal_id ? project.terminal_names[node.terminal_id] : undefined}
          projectId={project.id}
          path={path}
        />
      );
    case "split":
      return (
        <SplitLayout
          direction={node.direction}
          sizes={node.sizes}
          project={project}
          path={path}
        >
          {node.children}
        </SplitLayout>
      );
    case "tabs":
      return (
        <TabLayout
          activeTab={node.active_tab}
          project={project}
          path={path}
        >
          {node.children}
        </TabLayout>
      );
    case "app":
      return <AppPane appId={node.app_id} appKind={node.app_kind} />;
  }
}

function AppPane({ appId, appKind }: { appId: string | null; appKind: string }) {
  const { ws } = useApp();
  return (
    <KruhPane
      appId={appId}
      appKind={appKind}
      onAction={(action) => {
        if (appId) ws.sendAppAction(appId, action);
      }}
    />
  );
}
