import { invoke } from '@tauri-apps/api/core';

export interface BookRow {
  id: number;
  title: string;
  authors: string[];
  series: string | null;
  seriesIndex: number;
  tags: string[];
  formats: string[];
  uuid: string;
  coverPath: string | null;
  lastModified: string;
}

export interface DeviceStatus {
  connected: boolean;
  deviceName: string | null;
  port: number;
  discovery: boolean;
  onDevice: string[];
}

export interface SendProgress {
  current: number;
  total: number;
  title: string;
  error: string | null;
  done: boolean;
}

export interface Candidate {
  source: 'google' | 'openlibrary';
  title: string;
  authors: string[];
  publisher: string | null;
  pubdate: string | null;
  description: string | null;
  tags: string[];
  isbn: string | null;
  googleId: string | null;
  coverUrl: string | null;
}

export interface SearchResult {
  candidates: Candidate[];
  errors: string[];
}

export interface AddReport {
  added: number;
  ids: number[];
  skipped: { file: string; reason: string }[];
}

export const ADD_EXTENSIONS = [
  'epub', 'kepub', 'pdf', 'mobi', 'azw', 'azw3', 'fb2', 'cbz', 'cbr', 'djvu', 'txt', 'rtf', 'docx', 'html', 'htm'
];

export const libraryPath = () => invoke<string>('library_path');
export const listBooks = () => invoke<BookRow[]>('list_books');
export const addBooks = (paths: string[]) => invoke<AddReport>('add_books', { paths });
export const deviceStatus = () => invoke<DeviceStatus>('device_status');
export const sendBooks = (ids: number[]) => invoke<void>('send_books', { ids });
export const searchMetadata = (id: number) => invoke<SearchResult>('search_metadata', { id });
export const applyMetadata = (id: number, candidate: Candidate, meta: boolean, cover: boolean) =>
  invoke<void>('apply_metadata', { id, candidate, meta, cover });

export const autoMetadata = (ids: number[]) =>
  invoke<{ updated: number; failed: string[] }>('auto_metadata', { ids });
