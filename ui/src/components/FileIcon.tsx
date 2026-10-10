import { memo } from 'react'

function getIconKind(filename: string): string {
  const i = filename.lastIndexOf('.')
  const ext = i <= 0 || i === filename.length - 1 ? '' : filename.slice(i + 1).toLowerCase()

  if (['mp4', 'mkv', 'webm', 'avi', 'mov', 'flv', 'wmv'].includes(ext)) return 'video'
  if (['mp3', 'wav', 'flac', 'm4a', 'ogg', 'aac'].includes(ext)) return 'audio'
  if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp', 'ico'].includes(ext)) return 'image'
  if (['zip', 'rar', '7z', 'tar', 'gz', 'bz2', 'xz'].includes(ext)) return 'archive'
  if (['exe', 'msi', 'apk', 'dmg', 'appimage', 'deb', 'rpm'].includes(ext)) return 'executable'
  if (['pdf'].includes(ext)) return 'pdf'
  if (['doc', 'docx', 'txt', 'md', 'rtf', 'csv', 'xls', 'xlsx', 'ppt', 'pptx'].includes(ext)) return 'document'
  if (['iso', 'img', 'vhd'].includes(ext)) return 'disk'
  return 'unknown'
}

export const FileIcon = memo(function FileIcon({ filename, className = '' }: { filename: string; className?: string }) {
  const kind = getIconKind(filename)

  let iconSvg
  switch (kind) {
    case 'video':
      // Media player icon (film strip)
      iconSvg = (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <rect x="2" y="2" width="20" height="20" rx="2.18" ry="2.18"></rect>
          <line x1="7" y1="2" x2="7" y2="22"></line>
          <line x1="17" y1="2" x2="17" y2="22"></line>
          <line x1="2" y1="12" x2="22" y2="12"></line>
          <line x1="2" y1="7" x2="7" y2="7"></line>
          <line x1="2" y1="17" x2="7" y2="17"></line>
          <line x1="17" y1="17" x2="22" y2="17"></line>
          <line x1="17" y1="7" x2="22" y2="7"></line>
        </svg>
      )
      break
    case 'audio':
      iconSvg = (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M9 18V5l12-2v13"></path>
          <circle cx="6" cy="18" r="3"></circle>
          <circle cx="18" cy="16" r="3"></circle>
        </svg>
      )
      break
    case 'archive':
      // Looks somewhat like WinRAR/Zip archive
      iconSvg = (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <rect x="3" y="4" width="18" height="18" rx="2" ry="2"></rect>
          <line x1="12" y1="4" x2="12" y2="22"></line>
          <path d="M8 4v18"></path>
          <path d="M16 4v18"></path>
          <rect x="7" y="10" width="10" height="4" rx="1"></rect>
        </svg>
      )
      break
    case 'image':
      iconSvg = (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
          <circle cx="8.5" cy="8.5" r="1.5"></circle>
          <polyline points="21 15 16 10 5 21"></polyline>
        </svg>
      )
      break
    case 'executable':
      // Cog / App icon
      iconSvg = (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <rect x="2" y="4" width="20" height="16" rx="2"></rect>
          <path d="M10 8v8l5-4-5-4z"></path>
        </svg>
      )
      break
    case 'pdf':
      iconSvg = (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
          <polyline points="14 2 14 8 20 8"></polyline>
          <path d="M9 15v-4"></path>
          <path d="M12 15v-4"></path>
          <path d="M15 15v-4"></path>
          <path d="M9 11h6"></path>
        </svg>
      )
      break
    case 'document':
      iconSvg = (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
          <polyline points="14 2 14 8 20 8"></polyline>
          <line x1="16" y1="13" x2="8" y2="13"></line>
          <line x1="16" y1="17" x2="8" y2="17"></line>
          <polyline points="10 9 9 9 8 9"></polyline>
        </svg>
      )
      break
    case 'disk':
      iconSvg = (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <circle cx="12" cy="12" r="10"></circle>
          <circle cx="12" cy="12" r="3"></circle>
        </svg>
      )
      break
    default:
      // Generic File
      iconSvg = (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"></path>
          <polyline points="13 2 13 9 20 9"></polyline>
        </svg>
      )
      break
  }

  return (
    <span className={`file-icon-svg kind-${kind} ${className}`} aria-hidden>
      {iconSvg}
    </span>
  )
})
