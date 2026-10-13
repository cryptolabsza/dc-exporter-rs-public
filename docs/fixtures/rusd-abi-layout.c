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
#include <nv-unix-nvos-params-wrappers.h>
#define CHECK(T,S) _Static_assert(sizeof(T)==S,#T)
CHECK(NV00DE_SHARED_DATA,3144); CHECK(NVOS21_PARAMETERS,32); CHECK(NVOS54_PARAMETERS,32); CHECK(NVOS00_PARAMETERS,16); CHECK(NVOS33_PARAMETERS,48); CHECK(nv_ioctl_nvos33_parameters_with_fd,56); CHECK(NV0080_ALLOC_PARAMETERS,56); CHECK(NV0000_ALLOC_PARAMETERS,120); CHECK(nv_pci_info_t,12); CHECK(nv_ioctl_card_info_t,72); CHECK(NV0000_CTRL_GPU_ATTACH_IDS_PARAMS,132); CHECK(NV0000_CTRL_GPU_GET_UUID_INFO_PARAMS,272);
_Static_assert(offsetof(NV00DE_SHARED_DATA,temperatures)+sizeof(RUSD_TEMPERATURE)==2328,"memory timestamp");
_Static_assert(offsetof(NV00DE_SHARED_DATA,temperatures)+sizeof(RUSD_TEMPERATURE)+offsetof(RUSD_TEMPERATURE,temperature)==2336,"memory value");
_Static_assert(offsetof(nv_ioctl_card_info_t,minor_number)==56,"minor");
CHECK(NVOS34_PARAMETERS,32); CHECK(NV2080_ALLOC_PARAMETERS,4);
_Static_assert(NV01_ROOT_CLIENT==0x41,"root client class");
_Static_assert(NV20_SUBDEVICE_0==0x2080,"subdevice class");
_Static_assert(NV00DE_RUSD_POLL_THERMAL==0x10,"thermal-only mask");
int main(void){return 0;}
