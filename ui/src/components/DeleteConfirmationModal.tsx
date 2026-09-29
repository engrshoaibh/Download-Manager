import { useState } from 'react'
import type { DownloadJob } from '../hooks/useDownloads'

export function DeleteConfirmationModal({
  jobs,
  onConfirm,
  onCancel,
}: {
  jobs: DownloadJob[]
  onConfirm: (deleteFile: boolean) => void
  onCancel: () => void
}) {
  const [deleteFile, setDeleteFile] = useState(false)

  const text = jobs.length === 1
    ? `Are you sure you want to delete "${jobs[0].filename}"?`
    : `Are you sure you want to delete ${jobs.length} selected downloads?`

  return (
    <div className="modal-overlay" onClick={e => e.target === e.currentTarget && onCancel()}>
      <div className="modal modal-sm" role="dialog" aria-modal="true" aria-labelledby="delete-title">
        <div className="modal-header">
          <h2 className="modal-title" id="delete-title">Confirm Deletion</h2>
          <button className="btn btn-ghost btn-sm btn-icon" onClick={onCancel} aria-label="Close">✕</button>
        </div>
        <div className="confirm-body">
          <p className="confirm-copy">{text}</p>
          <label className="check-row">
            <input
              type="checkbox"
              checked={deleteFile}
              onChange={e => setDeleteFile(e.target.checked)}
            />
            Also delete the file from disk
          </label>
        </div>
        <div className="modal-footer">
          <button className="btn btn-ghost" onClick={onCancel}>Cancel</button>
          <button className="btn btn-danger" onClick={() => onConfirm(deleteFile)}>Delete</button>
        </div>
      </div>
    </div>
  )
}
