import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { EMPTY_PROFILE, MAX_AVATAR_URL, parseProfile, prepareAvatar, validAvatar } from './profile';
import { ProfileAvatar } from './ui/Profile';

const png = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB';
describe('browser-local profile admission', () => {
  it('keeps a small raster photo and a trimmed display name without accepting extra fields', () => {
    expect(parseProfile(JSON.stringify({ displayName: '  Owner  ', avatarDataUrl: png }))).toEqual({ displayName: 'Owner', avatarDataUrl: png });
    expect(parseProfile(JSON.stringify({ displayName: 'Owner', avatarDataUrl: null }))).toEqual({ displayName: 'Owner', avatarDataUrl: null });
    expect(parseProfile(JSON.stringify({ displayName: 'Owner', avatarDataUrl: png, token: 'untrusted' }))).toBe(EMPTY_PROFILE);
  });
  it('refuses executable, vector and remote image URLs from modified site storage', () => {
    for (const value of ['https://outside.example/avatar.png', 'javascript:alert(1)', 'data:image/svg+xml;base64,PHN2Zz4=', 'data:text/html;base64,PGltZz4=', '/private/file.png']) {
      expect(validAvatar(value)).toBe(false);
      expect(parseProfile(JSON.stringify({ displayName: 'Owner', avatarDataUrl: value }))).toBe(EMPTY_PROFILE);
    }
  });
  it('bounds corrupted storage, names and image data before use', () => {
    for (const raw of [null, '{', '[]', 'null', JSON.stringify({ displayName: 'x'.repeat(81), avatarDataUrl: null }), JSON.stringify({ displayName: 'line\nbreak', avatarDataUrl: null }), ' '.repeat(MAX_AVATAR_URL + 1025)]) expect(parseProfile(raw)).toBe(EMPTY_PROFILE);
    expect(validAvatar('data:image/png;base64,' + 'A'.repeat(MAX_AVATAR_URL))).toBe(false);
  });
  it('rejects unsupported or oversized file selections before image decoding', async () => {
    await expect(prepareAvatar({ type: 'image/svg+xml', size: 20 } as File)).rejects.toThrow('PNG, JPEG or WebP');
    await expect(prepareAvatar({ type: 'image/png', size: 5 * 1024 * 1024 + 1 } as File)).rejects.toThrow('5 MB');
    await expect(prepareAvatar({ type: 'image/png', size: 0 } as File)).rejects.toThrow('5 MB');
  });
  it('uses the uploaded raster or an accessible initial while escaping the owner label', () => {
    const photo = renderToStaticMarkup(<ProfileAvatar name="Owner" profile={{ displayName: '', avatarDataUrl: png }} />);
    expect(photo).toContain('Owner profile picture'); expect(photo).toContain(`src="${png}"`);
    const initial = renderToStaticMarkup(<ProfileAvatar name="<script>" profile={EMPTY_PROFILE} />);
    expect(initial).toContain('&lt;script&gt; profile picture'); expect(initial).not.toContain('<script>'); expect(initial).not.toContain('<img');
  });
});
