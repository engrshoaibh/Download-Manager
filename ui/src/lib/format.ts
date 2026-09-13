export function formatBytes(bytes: number, decimals = 1): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—'
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), sizes.length - 1)
  return `${(bytes / Math.pow(k, i)).toFixed(decimals)} ${sizes[i]}`
}

export function formatSpeed(bps: number): string {
  if (!bps || bps <= 0) return '—'
  return `${formatBytes(bps)}/s`
}

export function formatEta(secs: number | null): string {
  if (secs === null || secs <= 0 || !Number.isFinite(secs)) return '—'
  if (secs < 60) return `${Math.round(secs)}s`
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${Math.round(secs % 60)}s`
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`
}

export function formatDate(iso: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return '—'
  return d.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

export type CategoryId =
  | 'all'
  | 'unfinished'
  | 'finished'
  | 'compressed'
  | 'documents'
  | 'music'
  | 'video'
  | 'programs'
  | 'general'

export type FileKind = 'compressed' | 'documents' | 'music' | 'video' | 'programs' | 'general'

const EXT_MAP: Record<FileKind, string[]> = {
  compressed: ['zip', 'rar', '7z', 'tar', 'gz', 'tgz', 'bz2', 'xz', 'iso', 'cab', 'lz', 'zst'],
  documents: ['pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx', 'txt', 'rtf', 'odt', 'ods', 'csv', 'md'],
  music: ['mp3', 'flac', 'wav', 'aac', 'ogg', 'm4a', 'wma', 'opus', 'aiff'],
  video: ['mp4', 'mkv', 'avi', 'mov', 'wmv', 'webm', 'flv', 'm4v', 'mpeg', 'mpg', 'ts'],
  programs: ['exe', 'msi', 'dmg', 'apk', 'deb', 'rpm', 'appimage', 'jar', 'msix', 'bat', 'sh'],
  general: [],
}

export function getExtension(filename: string): string {
  const i = filename.lastIndexOf('.')
  if (i <= 0 || i === filename.length - 1) return ''
  return filename.slice(i + 1).toLowerCase()
}

export function getFileKind(filename: string): FileKind {
  const ext = getExtension(filename)
  if (!ext) return 'general'
  for (const kind of ['compressed', 'documents', 'music', 'video', 'programs'] as FileKind[]) {
    if (EXT_MAP[kind].includes(ext)) return kind
  }
  return 'general'
}

export function matchesCategory(
  filename: string,
  status: string,
  category: CategoryId,
): boolean {
  const unfinished = status !== 'completed'
  switch (category) {
    case 'all':
      return true
    case 'unfinished':
      return unfinished
    case 'finished':
      return status === 'completed'
    default:
      return getFileKind(filename) === category
  }
}
