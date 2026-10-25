import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { CAS_TIMEFRAMES, type SignalOptions } from '@/types/strategy_module'

interface Props {
  value: SignalOptions
  onChange: (v: SignalOptions) => void
}

const selectCls = 'h-9 w-full rounded-md border border-input bg-transparent px-2 text-sm'

function Toggle({
  id,
  label,
  help,
  checked,
  onChange,
}: {
  id: string
  label: string
  help: string
  checked: boolean
  onChange: (b: boolean) => void
}) {
  return (
    <label htmlFor={id} className="flex items-start gap-2">
      <input
        id={id}
        type="checkbox"
        className="mt-1"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span>
        <span className="font-medium">{label}</span>
        <span className="block text-xs text-muted-foreground">{help}</span>
      </span>
    </label>
  )
}

/** The Bridge 1.0.8: the "Advanced signal options" card of a signal strategy. */
export function SignalOptionsCard({ value: o, onChange }: Props) {
  const set = <K extends keyof SignalOptions>(k: K, v: SignalOptions[K]) =>
    onChange({ ...o, [k]: v })
  return (
    <Card data-testid="signal-options">
      <CardHeader className="pb-3">
        <CardTitle className="text-base">Advanced signal options</CardTitle>
        <CardDescription>
          All off by default: with nothing ticked the strategy just follows your BUY/SELL alerts.
          Times are IST.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-5 text-sm">
        <Toggle
          id="so-carry"
          label="Carry positions overnight"
          help="Open positions are kept into the next day until an exit/flip alert, a stop/target/trail or an expiry roll. Needs a positional strategy with product NRML (MIS is squared off by the broker)."
          checked={o.carry_overnight}
          onChange={(b) => set('carry_overnight', b)}
        />

        <div className="space-y-2">
          <Toggle
            id="so-roll"
            label="Roll on expiry day"
            help="On the held contract's expiry day at the roll time, close each leg expiring today and reopen it in the next expiry, re-resolving the strike by the leg's own rule (ATM offset or premium)."
            checked={o.expiry_roll}
            onChange={(b) => set('expiry_roll', b)}
          />
          {o.expiry_roll && (
            <div className="grid grid-cols-3 gap-3 pl-6">
              <div>
                <Label htmlFor="so-roll-time">Roll time (before 15:30)</Label>
                <Input
                  id="so-roll-time"
                  type="time"
                  value={o.roll_time}
                  onChange={(e) => set('roll_time', e.target.value)}
                />
              </div>
              <div>
                <Label htmlFor="so-reopen">Reopen</Label>
                <select
                  id="so-reopen"
                  className={selectCls}
                  value={o.reopen}
                  onChange={(e) => set('reopen', e.target.value as SignalOptions['reopen'])}
                >
                  <option value="next_session">Next session (engine default)</option>
                  <option value="immediate">Immediately</option>
                </select>
              </div>
              {o.reopen === 'next_session' && (
                <div>
                  <Label htmlFor="so-reopen-time">Reopen time</Label>
                  <Input
                    id="so-reopen-time"
                    type="time"
                    value={o.reopen_time}
                    onChange={(e) => set('reopen_time', e.target.value)}
                  />
                </div>
              )}
            </div>
          )}
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <Label htmlFor="so-min-days">Minimum days to expiry for a new entry</Label>
            <Input
              id="so-min-days"
              type="number"
              min={0}
              max={30}
              value={o.min_days_to_expiry}
              onChange={(e) => set('min_days_to_expiry', Number(e.target.value))}
            />
          </div>
          <div>
            <Label htmlFor="so-expiry-entry">Entry signal on expiry day</Label>
            <select
              id="so-expiry-entry"
              className={selectCls}
              value={o.expiry_day_entry}
              onChange={(e) =>
                set('expiry_day_entry', e.target.value as SignalOptions['expiry_day_entry'])
              }
            >
              <option value="same">Enter the expiring contract</option>
              <option value="defer">Defer to the next session</option>
            </select>
          </div>
          <p className="col-span-2 text-xs text-muted-foreground">
            A deferred entry waits for the next session's reopen time ({o.reopen_time}) and is
            dropped if an exit or opposite alert arrives first.
          </p>
        </div>

        <div className="space-y-2">
          <Toggle
            id="so-cas"
            label="Closing-session (CAS) exit"
            help="After the spot's last continuous bar, track a Supertrend on the traded option's premium and exit when it flips against the position. Off = ignore CAS (alerts only)."
            checked={o.cas_exit}
            onChange={(b) => set('cas_exit', b)}
          />
          {o.cas_exit && (
            <div className="grid grid-cols-3 gap-3 pl-6">
              <div className="col-span-3">
                <Label htmlFor="so-session-times">Session times</Label>
                <select
                  id="so-session-times"
                  className={selectCls}
                  value={o.session_times}
                  onChange={(e) =>
                    set('session_times', e.target.value as SignalOptions['session_times'])
                  }
                >
                  <option value="auto">Auto (exchange timetable)</option>
                  <option value="custom">Custom</option>
                </select>
                {o.session_times === 'auto' && (
                  <p className="mt-1 text-xs text-muted-foreground">
                    Until 6 Dec 2026: CAS from 15:15 (spot's last 1-minute bar 15:14) to F&amp;O
                    close 15:40. From 7 Dec 2026: 15:30 to 15:45, and 15:30 on the expiry day of the
                    contract held.
                  </p>
                )}
              </div>
              {o.session_times === 'custom' && (
                <>
                  <div>
                    <Label htmlFor="so-cas-start">CAS start (after spot's last bar)</Label>
                    <Input
                      id="so-cas-start"
                      type="time"
                      value={o.cas_start}
                      onChange={(e) => set('cas_start', e.target.value)}
                    />
                  </div>
                  <div>
                    <Label htmlFor="so-session-end">F&amp;O session end</Label>
                    <Input
                      id="so-session-end"
                      type="time"
                      value={o.session_end}
                      onChange={(e) => set('session_end', e.target.value)}
                    />
                  </div>
                  <div />
                </>
              )}
              <div>
                <Label htmlFor="so-cas-tf">Timeframe (min)</Label>
                <select
                  id="so-cas-tf"
                  className={selectCls}
                  value={o.cas_timeframe}
                  onChange={(e) => set('cas_timeframe', Number(e.target.value))}
                >
                  {CAS_TIMEFRAMES.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
              </div>
              <div>
                <Label htmlFor="so-cas-atr">Supertrend ATR</Label>
                <Input
                  id="so-cas-atr"
                  type="number"
                  min={1}
                  max={100}
                  value={o.cas_st_atr}
                  onChange={(e) => set('cas_st_atr', Number(e.target.value))}
                />
              </div>
              <div>
                <Label htmlFor="so-cas-mult">Multiplier</Label>
                <Input
                  id="so-cas-mult"
                  type="number"
                  step="0.1"
                  min={0.1}
                  max={20}
                  value={o.cas_st_mult}
                  onChange={(e) => set('cas_st_mult', Number(e.target.value))}
                />
              </div>
              <div>
                <Label htmlFor="so-cas-leg">Track premium of</Label>
                <select
                  id="so-cas-leg"
                  className={selectCls}
                  value={o.cas_leg}
                  onChange={(e) => set('cas_leg', e.target.value as SignalOptions['cas_leg'])}
                >
                  <option value="auto">Auto (sold leg, else bought)</option>
                  <option value="sell">Sold leg</option>
                  <option value="buy">Bought leg</option>
                </select>
              </div>
              <div>
                <Label htmlFor="so-cas-end">At session end</Label>
                <select
                  id="so-cas-end"
                  className={selectCls}
                  value={o.cas_at_end}
                  onChange={(e) => set('cas_at_end', e.target.value as SignalOptions['cas_at_end'])}
                >
                  <option value="hold">Hold (carry / normal exit)</option>
                  <option value="close">Close the position</option>
                </select>
              </div>
            </div>
          )}
        </div>
      </CardContent>
    </Card>
  )
}
