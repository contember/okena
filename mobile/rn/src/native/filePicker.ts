/**
 * filePicker.ts — pick a file on the phone and upload it to the server host,
 * where the terminal's process (or Claude) can read it.
 */

import {
  errorCodes,
  isErrorWithCode,
  keepLocalCopy,
  pick,
} from '@react-native-documents/picker';

import type { ConnId, OkenaNative } from './okena';

/** `file:///a/b%20c.png` → `/a/b c.png`; the native upload reads plain paths. */
export function fileUriToPath(uri: string): string {
  return decodeURIComponent(uri.replace(/^file:\/\//, ''));
}

/**
 * Let the user pick one file, copy it into the app's cache (Android hands out
 * `content://` URIs the upload cannot read) and upload it. Resolves to the
 * file's path on the server host, or `null` when the user cancelled. Throws
 * with a readable message otherwise.
 */
export async function pickAndUploadFile(
  native: Pick<OkenaNative, 'uploadFile'>,
  connId: ConnId,
): Promise<string | null> {
  let picked;
  try {
    [picked] = await pick({ mode: 'import' });
  } catch (error) {
    if (isErrorWithCode(error) && error.code === errorCodes.OPERATION_CANCELED)
      return null;
    throw error;
  }
  const [copy] = await keepLocalCopy({
    files: [{ uri: picked.uri, fileName: picked.name ?? 'upload' }],
    destination: 'cachesDirectory',
  });
  if (copy.status === 'error') throw new Error(copy.copyError);
  return native.uploadFile(connId, fileUriToPath(copy.localUri));
}
