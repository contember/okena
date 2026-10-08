import { Plugin } from "@opencode/plugin";
import type { SessionRequest } from "@opencode/plugin/promise/session";
import { hostname } from "node:os";
import { readBriefing } from "./bridge.js";
import { Okena, bindingKey, bindingSchema, type Binding } from "./rpc.js";

export default Plugin.define({
  id: "okena",
  async setup(ctx) {
    await ctx.rpc.register(Okena, {
      bind: async (binding) => {
        if (binding.hostname !== hostname()) {
          throw new Error(
            "Okena and the OpenCode server must run on the same host",
          );
        }
        // The TUI selects a new session ID before its create request completes.
        await ctx.storage.set(bindingKey(binding.sessionID), binding);
        return { bound: true };
      },
    });

    const inject = async (event: SessionRequest) => {
      let text: string;
      let binding: Binding | undefined;
      try {
        let id: string | undefined = event.sessionID;
        let stored: unknown;
        const visited = new Set<string>();
        while (id && !visited.has(id)) {
          visited.add(id);
          stored = await ctx.storage.get(bindingKey(id));
          if (stored !== undefined) break;
          const session = await ctx.session.get({ sessionID: id });
          id = session.parentID;
        }
        if (stored === undefined) return;
        const parsed = bindingSchema.safeParse(stored);
        if (!parsed.success) throw new Error("Invalid stored Okena binding");
        if (parsed.data.sessionID !== id)
          throw new Error("Stored Okena binding does not match its session");
        binding = parsed.data;
        // An unbound subagent uses its nearest bound ancestor's assignment.
        text = await readBriefing(binding);
      } catch (error) {
        console.error("Okena mission context:", error);
        text =
          "Okena mission context is currently unavailable. Do not treat a previous mission briefing as current.";
      }
      if (binding) {
        const environment = {
          OKENA_PROFILE: binding.profile,
          OKENA_TERMINAL_ID: binding.terminalID,
          XDG_CONFIG_HOME: binding.configHome,
        };
        text += `\nOpenCode shell tools run on a shared server. For Okena CLI calls, use this pane's routing environment (JSON values are data; null means unset): ${JSON.stringify(environment)}. For context queries, pass --agent opencode --session-id ${binding.sessionID}. Do not infer the pane or profile from the server environment.`;
      }
      event.system.push({ type: "text", text });
    };

    await ctx.session.hook("context", inject);
    await ctx.session.hook("compaction", inject);
  },
});
