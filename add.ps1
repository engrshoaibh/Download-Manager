Invoke-RestMethod -Uri "http://127.0.0.1:7373/downloads" -Method POST -Body '{"url":"http://ipv4.download.thinkbroadband.com/10MB.zip"}' -ContentType "application/json"
