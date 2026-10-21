import { useEffect, useRef, useState, type FormEvent } from 'react';
import { prepareAvatar, saveProfile, useProfile, type UserProfile } from '../profile';
import './profile.css';

export function ProfileAvatar({ name, className = '', profile }: { name: string; className?: string; profile: UserProfile }) {
  const [failed, setFailed] = useState<string | null>(null);
  return <span className={`avatar profile-avatar ${className}`} role="img" aria-label={`${name} profile picture`}>
    {profile.avatarDataUrl && profile.avatarDataUrl !== failed ? <img src={profile.avatarDataUrl} alt="" onError={() => setFailed(profile.avatarDataUrl)} /> : <span data-icon={name.trim().slice(0, 1).toUpperCase() || '?'} aria-hidden="true" />}
  </span>;
}

export function ProfileSettings({ onClose, onAccess }: { onClose: () => void; onAccess: () => void }) {
  const profile = useProfile(), [draft, setDraft] = useState<UserProfile>(profile), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const dialog = useRef<HTMLDialogElement>(null), uploadSequence = useRef(0);
  useEffect(() => { dialog.current?.showModal(); return () => { uploadSequence.current += 1; }; }, []);
  async function upload(file: File | undefined) {
    if (!file) return;
    const sequence = ++uploadSequence.current;
    setBusy(true); setError('');
    try { const avatarDataUrl = await prepareAvatar(file); if (sequence === uploadSequence.current) setDraft(value => ({ ...value, avatarDataUrl })); }
    catch (reason) { if (sequence === uploadSequence.current) setError(reason instanceof Error ? reason.message : 'Could not open this image.'); }
    finally { if (sequence === uploadSequence.current) setBusy(false); }
  }
  function save(event: FormEvent) {
    event.preventDefault();
    try { saveProfile(draft); onClose(); } catch (reason) { setError(reason instanceof Error ? reason.message : 'Could not save this profile.'); }
  }
  return <dialog className="profile-dialog" ref={dialog} aria-labelledby="profile-title" onCancel={onClose} onClick={event => {
    if (event.target === event.currentTarget) { const rect = event.currentTarget.getBoundingClientRect(); if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onClose(); }
  }}><form onSubmit={save} className="profile-body">
    <button className="modal-close" type="button" onClick={onClose} aria-label="Close profile"><span data-icon="×" aria-hidden="true" /></button>
    <div className="eyebrow">Your workspace</div><h2 id="profile-title">Your profile</h2>
    <p>Your photo appears in the owner’s hierarchy card and your profile menu.</p>
    <div className="profile-photo-row"><ProfileAvatar name={draft.displayName || 'Owner'} className="profile-preview" profile={draft} /><div>
      <label className={`secondary-button profile-upload ${busy ? 'is-busy' : ''}`}>Choose photo<input type="file" accept="image/png,image/jpeg,image/webp" disabled={busy} onChange={event => { const file = event.currentTarget.files?.[0]; void upload(file); event.currentTarget.value = ''; }} /></label>
      <button type="button" className="text-button" disabled={!draft.avatarDataUrl || busy} onClick={() => { uploadSequence.current += 1; setDraft(value => ({ ...value, avatarDataUrl: null })); setError(''); }}>Remove photo</button>
      <small>PNG, JPEG or WebP · up to 5 MB<br />Cropped to a square and resized to 256 × 256.</small>
    </div></div>
    <label className="profile-name" htmlFor="profile-name">Display name<input id="profile-name" value={draft.displayName} maxLength={80} placeholder="Your name" onChange={event => setDraft(value => ({ ...value, displayName: event.target.value }))} /></label>
    <p className="profile-local-note">Saved only in this browser. Your original photo is never uploaded to the fleet or sent to agents.</p>
    {busy && <p role="status">Preparing photo…</p>}{error && <p className="profile-error" role="alert">{error}</p>}
    <div className="profile-access-row"><span>Private dashboard access</span><button type="button" className="text-button" onClick={onAccess}>Session access <span data-icon="→" aria-hidden="true" /></button></div>
    <div className="modal-actions"><button type="button" className="secondary-button" onClick={onClose}>Cancel</button><button type="submit" className="primary-button" disabled={busy}>Save profile</button></div>
  </form></dialog>;
}
