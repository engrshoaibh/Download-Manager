Invoke-RestMethod -Uri "http://127.0.0.1:7373/downloads" -Method POST -Body '{"url":"https://nodejs.org/dist/v20.9.0/node-v20.9.0-linux-x64.tar.xz"}' -ContentType "application/json"
