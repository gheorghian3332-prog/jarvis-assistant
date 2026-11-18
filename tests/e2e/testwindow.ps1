# Our own test window for the screen-reading scenario (Notepad is banned in tests: it restores the user's files).
# Shows a large marker text for a few seconds. Usage: powershell -STA -File testwindow.ps1 -Marker "JARVIS TEST 4711" -Seconds 40
param([string]$Marker = 'JARVIS TEST 4711', [int]$Seconds = 40)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$form = New-Object System.Windows.Forms.Form
$form.Text = 'JARVIS E2E TEST WINDOW'
$form.StartPosition = 'CenterScreen'
$form.Size = New-Object System.Drawing.Size(900, 300)
$form.BackColor = [System.Drawing.Color]::White
$label = New-Object System.Windows.Forms.Label
$label.Text = $Marker
$label.Dock = 'Fill'
$label.TextAlign = 'MiddleCenter'
$label.ForeColor = [System.Drawing.Color]::Black
$label.Font = New-Object System.Drawing.Font('Arial', 32, [System.Drawing.FontStyle]::Regular)
$form.Controls.Add($label)
$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = $Seconds * 1000
$timer.Add_Tick({ $form.Close() })
$timer.Start()
$form.Add_Shown({ $form.Activate() })
[void]$form.ShowDialog()
