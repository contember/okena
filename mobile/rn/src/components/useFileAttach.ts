/**
 * useFileAttach.ts — the "attach a file" action shared by the Claude chat
 * composer and the terminal's compose sheet: pick, upload to the server host,
 * hand the server path to the caller.
 */

import { useCallback, useRef, useState } from 'react';

import { pickAndUploadFile } from '../native/filePicker';
import type { ConnId, OkenaNative } from '../native/okena';
import { nativeErrorMessage } from '../native/okena';

export interface FileAttach {
  /** Pick and upload; does nothing while an upload is running. */
  attach: () => void;
  uploading: boolean;
  /** Why the last attempt failed; cleared by the next one. */
  error: string | null;
}

export function useFileAttach(
  native: Pick<OkenaNative, 'uploadFile'>,
  connId: ConnId,
  onUploaded: (serverPath: string) => void,
): FileAttach {
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const running = useRef(false);
  const onUploadedRef = useRef(onUploaded);
  onUploadedRef.current = onUploaded;

  const attach = useCallback(() => {
    if (running.current) return;
    running.current = true;
    setUploading(true);
    setError(null);
    pickAndUploadFile(native, connId)
      .then((path) => {
        if (path !== null) onUploadedRef.current(path);
      })
      .catch((failure: unknown) => setError(nativeErrorMessage(failure)))
      .finally(() => {
        running.current = false;
        setUploading(false);
      });
  }, [native, connId]);

  return { attach, uploading, error };
}

/** `text` with `path` appended as its own word. */
export function appendPath(text: string, path: string): string {
  const separator = text.length === 0 || /\s$/.test(text) ? '' : ' ';
  return `${text}${separator}${path} `;
}
