/* TRACEWEAVER: file-role=abi-verification; req=REQ-THERMAL-012; trace=TRACE-THERMAL-012; ver=VER-THERMAL-010 */
#include <stddef.h>
#include <nvos.h>
#include <class/cl0041.h>
#include <class/cl2080.h>
#include <ctrl/ctrl00de.h>
#include <class/cl0000.h>
#include <class/cl0080.h>
#include <class/cl00de.h>
#include <ctrl/ctrl0000/ctrl0000gpu.h>
#include <nv-ioctl.h>
#include <nv_escape.h>
#include <nv-ioctl-numbers.h>
#include <nv-unix-nvos-params-wrappers.h>
#define CHECK(T,S) _Static_assert(sizeof(T)==S,#T)
/* Compile against the matching pinned source tree; default preserves 590. */
#ifndef RUSD_ABI_PROFILE
#define RUSD_ABI_PROFILE 590
#endif
#if RUSD_ABI_PROFILE == 590
CHECK(NV00DE_SHARED_DATA,3144);
#elif RUSD_ABI_PROFILE == 595
CHECK(NV00DE_SHARED_DATA,3152);
#elif RUSD_ABI_PROFILE == 615
CHECK(NV00DE_SHARED_DATA,3184);
CHECK(RUSD_MEM_BANK_REMAP,32);
#else
#error "Only exact pinned 590.48.01, 595.99.02 and 615.71.09 ABI fixtures are defined"
#endif
CHECK(NVOS21_PARAMETERS,32); CHECK(NVOS54_PARAMETERS,32); CHECK(NVOS00_PARAMETERS,16); CHECK(NVOS33_PARAMETERS,48); CHECK(nv_ioctl_nvos33_parameters_with_fd,56); CHECK(NV0080_ALLOC_PARAMETERS,56); CHECK(NV0000_ALLOC_PARAMETERS,120); CHECK(nv_pci_info_t,12); CHECK(nv_ioctl_card_info_t,72); CHECK(NV0000_CTRL_GPU_ATTACH_IDS_PARAMS,132); CHECK(NV0000_CTRL_GPU_GET_UUID_INFO_PARAMS,272);
_Static_assert(offsetof(NV00DE_SHARED_DATA,temperatures)+sizeof(RUSD_TEMPERATURE)==2328,"memory timestamp");
_Static_assert(offsetof(NV00DE_SHARED_DATA,temperatures)+sizeof(RUSD_TEMPERATURE)+offsetof(RUSD_TEMPERATURE,temperature)==2336,"memory value");
_Static_assert(offsetof(nv_ioctl_card_info_t,minor_number)==56,"minor");
CHECK(NVOS34_PARAMETERS,32); CHECK(NV2080_ALLOC_PARAMETERS,4);
_Static_assert(NV01_ROOT_CLIENT==0x41,"root client class");
_Static_assert(NV20_SUBDEVICE_0==0x2080,"subdevice class");
_Static_assert(NV00DE_RUSD_POLL_THERMAL==0x10,"thermal-only mask");
CHECK(RUSD_TEMPERATURE,16);
CHECK(NV0000_CTRL_GPU_GET_ID_INFO_V2_PARAMS,32);
CHECK(NV0000_CTRL_GPU_DETACH_IDS_PARAMS,128);
_Static_assert(offsetof(NV00DE_SHARED_DATA,temperatures)==2312,"temperature base");
_Static_assert(RUSD_TEMPERATURE_TYPE_MEMORY==1,"memory sensor");
_Static_assert(_Generic((NvTemp)0, NvSFXP24_8: 1, default: 0),"signed24.8 NvTemp type");
_Static_assert(sizeof(NvTemp)==4 && (NvTemp)-1 < 0,"signed32 temperature representation");
_Static_assert(NV_ESC_CARD_INFO==200 && NV_ESC_REGISTER_FD==201,"linux escapes");
_Static_assert(NV_ESC_RM_ALLOC==0x2b && NV_ESC_RM_CONTROL==0x2a && NV_ESC_RM_FREE==0x29,"RM object escapes");
_Static_assert(NV_ESC_RM_MAP_MEMORY==0x4e && NV_ESC_RM_UNMAP_MEMORY==0x4f,"RM mapping escapes");
_Static_assert(NV0000_CTRL_CMD_GPU_GET_UUID_INFO==0x274 && NV0000_CTRL_CMD_GPU_GET_ID_INFO_V2==0x205,"identity commands");
_Static_assert(NV0000_CTRL_CMD_GPU_ATTACH_IDS==0x215 && NV0000_CTRL_CMD_GPU_DETACH_IDS==0x216,"target commands");
_Static_assert(NV00DE_CTRL_CMD_REQUEST_DATA_POLL==0xde0001,"poll command");
_Static_assert(RUSD_SEQ_START==0xff00000000000000ULL,"sequence boundary");
_Static_assert(offsetof(nv_ioctl_nvos33_parameters_with_fd,fd)==48,"map FD offset");
int main(void){return 0;}
