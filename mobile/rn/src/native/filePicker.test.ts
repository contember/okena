/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import { keepLocalCopy, pick } from '@react-native-documents/picker';

import { appendPath } from '../components/useFileAttach';
import { fileUriToPath, pickAndUploadFile } from './filePicker';

jest.mock('@react-native-documents/picker', () => {
  const actual = jest.requireActual('@react-native-documents/picker');
  return {
    ...actual,
    pick: jest.fn(async () => [
      { uri: 'content://doc/1', name: 'mockName.pdf' },
    ]),
    keepLocalCopy: jest.fn(async () => [
      {
        status: 'success',
        sourceUri: 'content://doc/1',
        localUri: 'file:///mock/uri/mockName.pdf',
      },
    ]),
  };
});

describe('fileUriToPath', () => {
  it('strips the scheme and decodes escapes', () => {
    expect(fileUriToPath('file:///data/cache/a%20b%23.png')).toBe(
      '/data/cache/a b#.png',
    );
  });
});

describe('appendPath', () => {
  it('adds the path as its own word, followed by a space', () => {
    expect(appendPath('', '/tmp/a.png')).toBe('/tmp/a.png ');
    expect(appendPath('look at', '/tmp/a.png')).toBe('look at /tmp/a.png ');
    expect(appendPath('look at ', '/tmp/a.png')).toBe('look at /tmp/a.png ');
  });
});

describe('pickAndUploadFile', () => {
  it('uploads the local copy of the picked file', async () => {
    const uploadFile = jest.fn(async () => '/tmp/okena-remote-paste-1.pdf');
    await expect(pickAndUploadFile({ uploadFile }, 'conn')).resolves.toBe(
      '/tmp/okena-remote-paste-1.pdf',
    );
    expect(uploadFile).toHaveBeenCalledWith('conn', '/mock/uri/mockName.pdf');
  });

  it('resolves to null when the user cancels', async () => {
    jest
      .mocked(pick)
      .mockRejectedValueOnce(
        Object.assign(new Error('cancelled'), { code: 'OPERATION_CANCELED' }),
      );
    const uploadFile = jest.fn(async () => '/tmp/x');
    await expect(pickAndUploadFile({ uploadFile }, 'conn')).resolves.toBeNull();
    expect(uploadFile).not.toHaveBeenCalled();
  });

  it('reports a failed local copy', async () => {
    jest
      .mocked(keepLocalCopy)
      .mockResolvedValueOnce([
        { status: 'error', sourceUri: 'content://x', copyError: 'no space' },
      ]);
    const uploadFile = jest.fn(async () => '/tmp/x');
    await expect(pickAndUploadFile({ uploadFile }, 'conn')).rejects.toThrow(
      'no space',
    );
    expect(uploadFile).not.toHaveBeenCalled();
  });
});
