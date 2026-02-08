import type { ApiLayoutNode, ApiProject } from "../api/types";
import { TerminalPane } from "./TerminalPane";
import { SplitLayout } from "./SplitLayout";
import { TabLayout } from "./TabLayout";

export function TerminalArea({
  layout,
  project,
}: {
  layout: ApiLayoutNode;
  project: ApiProject;
}) {
  return (
    <div className="h-full">
      <LayoutRenderer node={layout} project={project} />
    </div>
  );
}

export function LayoutRenderer({
  node,
  project,
}: {
  node: ApiLayoutNode;
  project: ApiProject;
}) {
  switch (node.type) {
    case "terminal":
      return (
        <TerminalPane
          terminalId={node.terminal_id}
          name={node.terminal_id ? project.terminal_names[node.terminal_id] : undefined}
        />
      );
    case "split":
      return (
        <SplitLayout
          direction={node.direction}
          sizes={node.sizes}
          project={project}
        >
          {node.children}
        </SplitLayout>
      );
    case "tabs":
      return (
        <TabLayout
          activeTab={node.active_tab}
          project={project}
        >
          {node.children}
        </TabLayout>
      );
  }
}
