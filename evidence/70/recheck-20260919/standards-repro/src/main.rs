use eneros_boot_protocol::{BootControl, BootSlot};
use eneros_drv_svc::{boot_guard::{BootWriteGuard, RunningIdentity, BOOTCTRL_START_LBA}, boot_install::BootInstall, block_cmd::{BlockBackend, InstallBackend}};
use eneros_business_services::{mgmt_rpc::*, mgmt_ops::{OpStatus, MgmtOpsState}};
struct Face { bytes: [u8; 4096], guard: BootWriteGuard }
impl BlockBackend for Face {
 fn read(&mut self, _: u64, buf: &mut [u8]) -> Result<(),()> {buf.copy_from_slice(&self.bytes[..buf.len()]); Ok(())}
 fn write(&mut self, _: u64, data: &[u8]) -> Result<(),()> {self.bytes.copy_from_slice(data); Ok(())}
 fn flush(&mut self)->Result<(),()> {Ok(())}
 fn health(&self)->(u64,u32,u8){(262144,512,0)}
}
impl InstallBackend for Face {
 fn install_ranges(&self)->(u64,u64,u64,u64){(self.guard.inactive_start_lba,self.guard.inactive_lbas,BOOTCTRL_START_LBA,2048)}
 fn running_identity(&self)->Option<RunningIdentity>{self.guard.running}
}
struct Exec { calls: usize, persist_calls: usize, async_mode: bool, terminal_fail: bool, durable: Option<MgmtOpsState> }
impl Exec {fn new(async_mode:bool, terminal_fail:bool)->Self{Self{calls:0,persist_calls:0,async_mode,terminal_fail,durable:None}}}
impl ActionExecutor for Exec {
 fn execute(&mut self,_:&MgmtAction,_:&AuthenticatedSession,_:&[u8],_:u64)->ActionResult{self.calls+=1;ActionResult{status:if self.async_mode {RpcStatus::Accepted}else{RpcStatus::Completed},detail:"actual-effect".into(),payload:None}}
 fn ops_storage_available(&mut self)->bool{true}
 fn persist_ops_state(&mut self,state:&MgmtOpsState)->bool{self.persist_calls+=1;if self.terminal_fail && self.persist_calls>1 {false}else{self.durable=Some(state.clone());true}}
 fn observe_operation(&mut self,_:&eneros_business_services::mgmt_ops::OperationRecord,_:u64)->Option<(OpStatus,String)>{Some((OpStatus::Completed,"observed-effect".into()))}
}
fn main() {
 let mut bc=BootControl::factory([7;16],1,[7;16]); bc.slot_a.tries_remaining=1;
 let (booting,persisted)=bc.prepare_attempt().unwrap(); assert_eq!(booting.slot,BootSlot::A);
 let disk=persisted.unwrap().encode().unwrap();
 let running=RunningIdentity{is_a:true,security_version:1,release_id:[7;16]};
 let guard=BootWriteGuard::resolve_with_running(&disk,&disk,Some(running)).unwrap();
 let (a_lba,_)=eneros_ota_ab::boot_disk::lba_range_of(eneros_ota_ab::boot_disk::BootPartition::BootA);
 assert!(!guard.install_write_allowed(a_lba,1));
 let mut face=Face{bytes:disk,guard};
 assert!(BootInstall::new(&mut face).confirm_running(true,1,[7;16]).is_ok());
 println!("S1 last-attempt protection+confirmation: fixed");
 let empty=BootWriteGuard::resolve_with_running(&disk,&disk,None).unwrap();
 println!("S1 missing-identity raw BOOTCTRL write allowed={}",empty.install_write_allowed(BOOTCTRL_START_LBA,1));
 let session=AuthenticatedSession{principal:"owner".into(),roles:vec!["admin".into(),"viewer".into()],tenant_scope:1,channel_binding:"tls".into(),expires_at_ms:10000};
 let mut rpc=ManagementRpc::new(Exec::new(false,true));
 let first=rpc.handle(&session,123,"maintenance-enter",b"",1);
 let replay=rpc.handle(&session,123,"maintenance-enter",b"",2);
 let query=rpc.handle(&session,999,"status",b"op:1",3);
 println!("S2 sync terminal-failure first={:?} replay={:?} query={:?} durable={:?} pending={} persist_calls={}",first.status,replay.status,query.status,rpc.executor.durable.as_ref().unwrap().ledger.get(1).unwrap().status,rpc.pending_persist(),rpc.executor.persist_calls);
 // recheck RS1 修复后行为：终态未确认落盘 ⇒ 首答/重放/查询全部出口
 // 不得回 Completed（盘上事实仍是 Running）；补写成功后才收敛。
 assert_eq!(first.status,RpcStatus::Failed);
 assert_eq!(replay.status,RpcStatus::Failed); assert!(replay.detail.contains("terminal-persist-pending"));
 assert_eq!(query.status,RpcStatus::Failed); assert!(query.detail.contains("terminal-persist-pending"));
 assert_eq!(rpc.executor.durable.as_ref().unwrap().ledger.get(1).unwrap().status,OpStatus::Running);
 assert!(rpc.pending_persist());
 let mut rpc=ManagementRpc::new(Exec::new(true,true));
 let first=rpc.handle(&session,124,"agent-restart",b"svc-ota",1);
 let query=rpc.handle(&session,999,"status",b"op:1",2);
 println!("S2 async observation first={:?} query={:?} durable={:?} pending={} persist_calls={}",first.status,query.status,rpc.executor.durable.as_ref().unwrap().ledger.get(1).unwrap().status,rpc.pending_persist(),rpc.executor.persist_calls);
 // recheck RS1 修复后行为：异步观测收口的终态在回复之前确认落盘——
 // 提交失败 ⇒ 本次回复即非 Completed（不再「先回 Completed 再兜底」）。
 assert_eq!(first.status,RpcStatus::Accepted);
 assert_eq!(query.status,RpcStatus::Failed); assert!(query.detail.contains("terminal-persist-pending"));
 assert_eq!(rpc.executor.persist_calls,2);
 let mut rpc=ManagementRpc::new(Exec::new(false,false));
 let enter=rpc.handle(&session,125,"maintenance-enter",b"",1);
 let exit=rpc.handle(&session,125,"maintenance-exit",b"",2);
 assert_eq!(enter.status,RpcStatus::Completed); assert_eq!(exit.status,RpcStatus::Failed); assert!(exit.detail.contains("idempotency-conflict"));
 println!("S3 different action same-id: fixed, executed={}",rpc.executor.calls);
}
