import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App.tsx'
import { DownloadProgressDialog } from './components/DownloadProgressDialog'
import './index.css'

const searchParams = new URLSearchParams(window.location.search)
const dialog = searchParams.get('dialog')

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    {dialog === 'progress' ? <DownloadProgressDialog /> : <App />}
  </React.StrictMode>,
)
