import { Rpc } from "@opencode/plugin/rpc";
import { z } from "zod";
import { isAbsolute } from "node:path";

export const sessionID = z
  .string()
  .max(128)
  .regex(/^ses_[A-Za-z0-9]+$/);

export const bindingSchema = z
  .object({
    sessionID,
    terminalID: z
      .string()
      .min(1)
      .max(128)
      .regex(/^[A-Za-z0-9_-]+$/),
    profile: z
      .string()
      .min(1)
      .max(128)
      .regex(/^[A-Za-z0-9_-]+$/),
    configHome: z.string().max(4096).refine(isAbsolute).nullable(),
    hostname: z.string().min(1).max(255),
  })
  .strict();

export type Binding = z.infer<typeof bindingSchema>;
export function bindingKey(id: string): string {
  return `binding/${id}`;
}

export const Okena = Rpc.define({
  id: "okena",
  methods: {
    bind: {
      input: bindingSchema,
      output: z.object({ bound: z.boolean() }),
    },
  },
  events: {},
});
