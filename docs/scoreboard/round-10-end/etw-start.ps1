logman stop aquakeylat -ets 2>&1 | Out-Null
Remove-Item "C:\Users\sander\AppData\Local\Temp\aqua-key-latency.etl" -ErrorAction SilentlyContinue
logman create trace aquakeylat -p "Microsoft-Windows-DXGI" 0xFFFFFFFFFFFFFFFF 5 -ct system -o "C:\Users\sander\AppData\Local\Temp\aqua-key-latency.etl" -ets -bs 1024 -nb 64 256 | Out-Null
