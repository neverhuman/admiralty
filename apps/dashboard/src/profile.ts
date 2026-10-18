import { useSyncExternalStore } from 'react';

export type UserProfile = { displayName: string; avatarDataUrl: string | null };
export const PROFILE_KEY = 'admiralty.profile.v1';
export const MAX_AVATAR_URL = 360_000;
export const EMPTY_PROFILE: UserProfile = Object.freeze({ displayName: '', avatarDataUrl: null });
export const AVATAR_SIZE = 256;
const listeners = new Set<() => void>();
let cachedRaw: string | null = null;
let cachedProfile: UserProfile = EMPTY_PROFILE;

export function validAvatar(value: unknown): value is string {
  return typeof value === 'string' && value.length <= MAX_AVATAR_URL &&
    /^data:image\/(?:png|jpeg|webp);base64,[A-Za-z0-9+/]+={0,2}$/.test(value);
}
export function parseProfile(raw: string | null): UserProfile {
  if (!raw || raw.length > MAX_AVATAR_URL + 1024) return EMPTY_PROFILE;
  try {
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== 'object' || Array.isArray(value)) return EMPTY_PROFILE;
    const record = value as Record<string, unknown>;
    if (Object.keys(record).some(key => !['displayName', 'avatarDataUrl'].includes(key)) ||
        typeof record.displayName !== 'string' || record.displayName.length > 80 || /[\u0000-\u001f\u007f]/.test(record.displayName) ||
        !(record.avatarDataUrl === null || validAvatar(record.avatarDataUrl))) return EMPTY_PROFILE;
    return { displayName: record.displayName.trim(), avatarDataUrl: record.avatarDataUrl };
  } catch { return EMPTY_PROFILE; }
}
function snapshot(): UserProfile {
  let raw: string | null;
  try { raw = window.localStorage.getItem(PROFILE_KEY); } catch { return cachedProfile; }
  if (raw !== cachedRaw) { cachedRaw = raw; cachedProfile = parseProfile(raw); }
  return cachedProfile;
}
function subscribe(listener: () => void) {
  listeners.add(listener);
  const update = (event: StorageEvent) => { if (event.key === PROFILE_KEY || event.key === null) listener(); };
  window.addEventListener('storage', update);
  return () => { listeners.delete(listener); window.removeEventListener('storage', update); };
}
export function useProfile(): UserProfile { return useSyncExternalStore(subscribe, snapshot, () => EMPTY_PROFILE); }
export function saveProfile(profile: UserProfile): void {
  const raw = JSON.stringify(profile), clean = parseProfile(raw);
  if (clean === EMPTY_PROFILE) throw new Error('Choose a supported profile name and image.');
  try { window.localStorage.setItem(PROFILE_KEY, raw); }
  catch { throw new Error('This browser could not save the profile. Check available site storage.'); }
  cachedRaw = raw; cachedProfile = clean; listeners.forEach(listener => listener());
}

/** Rasterize locally into a small square; metadata and the original file are not retained. */
export async function prepareAvatar(file: File): Promise<string> {
  if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) throw new Error('Choose a PNG, JPEG or WebP image.');
  if (file.size === 0 || file.size > 5 * 1024 * 1024) throw new Error('Choose an image smaller than 5 MB.');
  const url = URL.createObjectURL(file);
  try {
    const image = new Image();
    await new Promise<void>((resolve, reject) => { image.onload = () => resolve(); image.onerror = () => reject(new Error('This image could not be opened.')); image.src = url; });
    if (!image.naturalWidth || !image.naturalHeight || image.naturalWidth * image.naturalHeight > 20_000_000) throw new Error('Choose an image with at most 20 megapixels.');
    const canvas = document.createElement('canvas'); canvas.width = AVATAR_SIZE; canvas.height = AVATAR_SIZE;
    const context = canvas.getContext('2d'); if (!context) throw new Error('Image resizing is unavailable in this browser.');
    const side = Math.min(image.naturalWidth, image.naturalHeight);
    context.drawImage(image, (image.naturalWidth - side) / 2, (image.naturalHeight - side) / 2, side, side, 0, 0, AVATAR_SIZE, AVATAR_SIZE);
    const result = canvas.toDataURL('image/png');
    if (!validAvatar(result)) throw new Error('The resized image is too large. Choose another image.');
    return result;
  } finally { URL.revokeObjectURL(url); }
}
