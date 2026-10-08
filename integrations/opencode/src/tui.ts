import { Plugin } from "@opencode/plugin/tui";
import { createEffect } from "solid-js";
import { hostname } from "node:os";
import { Okena, bindingSchema } from "./rpc.js";
import { paneSchema, reportStatus, type AgentState } from "./status.js";

export default Plugin.define({
  id: "okena.tui",
  setup(ctx) {
    const terminalID = process.env.OKENA_TERMINAL_ID;
    const profile = process.env.OKENA_PROFILE;
    if (!terminalID || !profile) return;
    const parsed = paneSchema.safeParse({
      terminalID,
      profile,
      configHome: process.env.XDG_CONFIG_HOME ?? null,
      ttyFile: process.env.OKENA_TTY_FILE,
      tty: process.env.OKENA_TTY,
    });
    if (!parsed.success) {
      console.error("Invalid Okena pane environment:", parsed.error);
      return;
    }
    const pane = parsed.data;
    const okena = ctx.client.rpc(Okena);
    let selected: string | undefined;
    const inputRequests = new Set<string>();
    let pending = Promise.resolve();
    const report = (id: string, state: AgentState) => {
      pending = pending
        .then(() => reportStatus(pane, id, state))
        .catch((error) => {
          console.error("Okena agent status:", error);
        });
    };
    const input = (id: string, request: string, waiting: boolean) => {
      if (id !== selected) return;
      if (waiting) inputRequests.add(request);
      else inputRequests.delete(request);
      report(
        id,
        inputRequests.size > 0
          ? "blocked"
          : ctx.data.session.status(id) === "running"
            ? "working"
            : "idle",
      );
    };
    const bind = async (id: string) => {
      const binding = bindingSchema.parse({
        terminalID: pane.terminalID,
        profile: pane.profile,
        configHome: pane.configHome,
        sessionID: id,
        hostname: hostname(),
      });
      await okena.bind(binding, {
        location: ctx.data.session.get(id)?.location ?? ctx.location,
      });
      const known = ctx.data.session.get(id) !== undefined;
      if (known) {
        await Promise.all([
          ctx.data.session.permission.sync(id),
          ctx.data.session.form.sync(id),
        ]);
      }
      if (selected === id) {
        if (known) inputRequests.clear();
        for (const request of ctx.data.session.permission.list(id) ?? []) {
          inputRequests.add(`permission/${request.id}`);
        }
        for (const form of ctx.data.session.form.list(id) ?? []) {
          inputRequests.add(`form/${form.id}`);
        }
        report(
          id,
          inputRequests.size > 0
            ? "blocked"
            : ctx.data.session.status(id) === "running"
              ? "working"
              : "clear",
        );
      }
    };
    const stopSlot = ctx.ui.slot({
      append: "app",
      render: () => {
        ctx.keymap.layer(() => ({
          commands: [
            {
              id: "okena.bind",
              title: "Bind this session to the Okena pane",
              group: "Okena",
              palette: true,
              slash: { name: "okena-bind" },
              run: async () => {
                if (selected) await bind(selected);
              },
            },
          ],
        }));
        createEffect(() => {
          const route = ctx.ui.router.current();
          const id = route.type === "session" ? route.sessionID : undefined;
          if (id === selected) return;
          if (selected) report(selected, "clear");
          inputRequests.clear();
          selected = id;
          if (id)
            void bind(id).catch((error) =>
              console.error("Okena session binding:", error),
            );
        });
        return null;
      },
    });
    const stops = [
      ctx.data.on("session.execution.started", (event) => {
        if (event.data.sessionID === selected)
          report(
            event.data.sessionID,
            inputRequests.size > 0 ? "blocked" : "working",
          );
      }),
      ctx.data.on("session.execution.succeeded", (event) => {
        if (event.data.sessionID === selected)
          report(event.data.sessionID, "done");
      }),
      ctx.data.on("session.execution.failed", (event) => {
        if (event.data.sessionID === selected)
          report(event.data.sessionID, "idle");
      }),
      ctx.data.on("session.execution.interrupted", (event) => {
        if (event.data.sessionID === selected)
          report(event.data.sessionID, "idle");
      }),
      ctx.data.on("permission.asked", (event) => {
        input(event.data.sessionID, `permission/${event.data.id}`, true);
      }),
      ctx.data.on("permission.replied", (event) => {
        input(
          event.data.sessionID,
          `permission/${event.data.requestID}`,
          false,
        );
      }),
      ctx.data.on("form.created", (event) => {
        input(event.data.form.sessionID, `form/${event.data.form.id}`, true);
      }),
      ctx.data.on("form.replied", (event) => {
        input(event.data.sessionID, `form/${event.data.id}`, false);
      }),
      ctx.data.on("form.cancelled", (event) => {
        input(event.data.sessionID, `form/${event.data.id}`, false);
      }),
    ];
    return async () => {
      stopSlot();
      for (const stop of stops) stop();
      if (selected) report(selected, "clear");
      await pending;
    };
  },
});
