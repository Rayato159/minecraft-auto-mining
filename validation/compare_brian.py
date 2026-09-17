"""Independent Brian2 oracle. Only validation uses Python; the controller is all Rust.
Install brian2==2.10.1 into tmp/brian-validation, then run after `miner brain-trace`.
The source repository's reset mentions an undeclared `w`; omit that unused assignment.
"""
from pathlib import Path
import sys, json
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tmp/brian-validation'))
import numpy as np
import brian2 as b
b.prefs.codegen.target='numpy'
b.defaultclock.dt=.1*b.ms
b.start_scope()
n=b.NeuronGroup(19,'''dv/dt=(-52*mV-v+g)/(20*ms):volt (unless refractory)
dg/dt=-g/(5*ms):volt (unless refractory)
rfc:second''',threshold='v>-45*mV',reset='v=-52*mV;g=0*mV',refractory='rfc',method='linear')
n.v=-52*b.mV;n.g=0*b.mV;n.rfc=2.2*b.ms;n.rfc[0]=0*b.ms
s=b.Synapses(n,n,'w:volt',on_pre='g_post+=w',delay=1.8*b.ms)
s.connect(i=[0,0,1,2],j=[1,2,2,1]);s.w=np.array([15.125,-4.95,40.,-20.])*b.mV
times=np.r_[np.arange(0,400,5),500]*.1*b.ms
source=b.SpikeGeneratorGroup(2,np.r_[np.zeros(80,dtype=int),1],times)
# model.py uses PoissonInput(target_var='v'). The deterministic spike train
# replaces only its random event times, preserving delivery variable/schedule.
stim=b.Synapses(source,n,'w:volt',on_pre='v_post+=w');stim.connect(i=[0,1],j=[0,1]);stim.w=[68.75,80]*b.mV
monitor=b.StateMonitor(n,['v','g'],record=True,when='end');spikes=b.SpikeMonitor(n)
b.run(100*b.ms)
rust=json.loads((ROOT/'runtime/brain-reference-rust.json').read_text())
rv=np.array([r['v'] for r in rust]).T;rg=np.array([r['g'] for r in rust]).T
errors={'v_max_error_mV':float(np.max(np.abs(rv-monitor.v/b.mV))),'g_max_error_mV':float(np.max(np.abs(rg-monitor.g/b.mV)))}
actual=[(r['tick'],i) for r in rust for i in r['spikes']]
expected=sorted((round(float(t/b.ms)*10),int(i)) for t,i in zip(spikes.t,spikes.i))
result={'brian_version':b.__version__,'neurons':19,'simulation_ms':100,'spike_count':len(expected),'spike_times_identical':actual==expected,**errors}
(ROOT/'runtime/brain-reference-validation.json').write_text(json.dumps(result,indent=2))
print(json.dumps(result,indent=2))
assert actual==expected, (actual[:25],expected[:25])
assert errors['v_max_error_mV']<.005 and errors['g_max_error_mV']<.005, errors
