use eneros_boot_protocol::{BootControl, BootSlot};
use eneros_drv_svc::{boot_guard::{BootWriteGuard, BOOTCTRL_START_LBA}, boot_install::BootInstall, block_cmd::{BlockBackend, InstallBackend}};
use eneros_business_services::mgmt_rpc::*;
struct Face { bytes: [u8; 4096], guard: BootWriteGuard }
impl BlockBackend for Face {
 fn read(&mut self, _: u64, buf: &mut [u8]) -> Result<(),()> {buf.copy_from_slice(&self.bytes[..buf.len()]); Ok(())}
 fn write(&mut self, _: u64, data: &[u8]) -> Result<(),()> {self.bytes.copy_from_slice(data); Ok(())}
 fn flush(&mut self)->Result<(),()> {Ok(())}
 fn health(&self)->(u64,u32,u8){(262144,512,0)}
}
impl InstallBackend for Face {fn install_ranges(&self)->(u64,u64,u64,u64){(self.guard.inactive_start_lba,self.guard.inactive_lbas,BOOTCTRL_START_LBA,2048)}}
struct Exec(usize);
impl ActionExecutor for Exec {fn execute(&mut self,_:&MgmtAction,_:&AuthenticatedSession,_:&[u8],_:u64)->ActionResult{self.0+=1;ActionResult{status:RpcStatus::Completed, detail:"effect happened".into(),payload:None}}}
fn main() {
 let mut bc=BootControl::factory([7;16],1,[7;16]);
 bc.slot_a.tries_remaining=1;
 let (booting,persisted)=bc.prepare_attempt().unwrap();
 assert_eq!(booting.slot,BootSlot::A);
 let disk=persisted.unwrap().encode().unwrap();
 let guard=BootWriteGuard::resolve(&disk,&disk).unwrap();
 let (a_lba,_)=eneros_ota_ab::boot_disk::lba_range_of(eneros_ota_ab::boot_disk::BootPartition::BootA);
 println!("last trial: booting={:?}, guard.active_is_a={}, running_slot_write_allowed={}",booting.slot,guard.active_is_a,guard.install_write_allowed(a_lba,1));
 assert!(!guard.active_is_a);
 assert!(guard.install_write_allowed(a_lba,1));
 let mut face=Face{bytes:disk,guard};
 let result=BootInstall::new(&mut face).confirm_running(true,1,[7;16]);
 println!("last trial confirmation={result:?}");
 assert_eq!(result,Err("confirm-slot-not-selected"));
 let session=AuthenticatedSession{principal:"owner".into(),roles:vec!["admin".into()],tenant_scope:1,channel_binding:"tls".into(),expires_at_ms:10000};
 let mut rpc=ManagementRpc::new(Exec(0));
 let enter=rpc.handle(&session,123,"maintenance-enter",b"",1);
 let exit=rpc.handle(&session,123,"maintenance-exit",b"",2);
 println!("different action same id+arg: enter={:?}, exit={:?}, executed_count={}, exit_detail={}",enter.status,exit.status,rpc.executor.0,exit.detail);
 assert_eq!(rpc.executor.0,1);
 assert_eq!(exit.status,RpcStatus::Completed);
}
