/* SpaceRecycle - delete interception minifilter.
 *
 * Milestone 1: prove the chain. The filter loads, sees every delete attempt,
 * and reports the path to a user-mode listener. It deliberately allows
 * everything: policy comes later, and a filter that starts out blocking is a
 * filter that bricks a machine the first time it is wrong.
 *
 * Why a minifilter rather than something in user mode: nothing in user mode is
 * offered a chance to say no to a delete. Copy hooks are called for folders
 * only. A deny-ACE or an open handle does stop a delete, but both break
 * replace-on-save (Word, most editors write a temp file and rename it over the
 * target, which needs DELETE on the target), and both surface as an OS error
 * rather than a question. Only the file system filter manager sees the
 * operation before it happens, on every path - Explorer, cmd, scripts, any
 * application - which is what "prompt the user or forbid it" requires.
 *
 * Targeting Windows 7 (`_WIN32_WINNT=0x0601`): a down-level driver loads on
 * newer systems too, and the Win7 VM is the only machine this can be tested on.
 * Everything used here is Vista-era, so it binds against Win7's fltmgr.sys.
 */

#include <fltkernel.h>
#include <ntstrsafe.h>

/* Not exposed by the Windows 7 headers: the value is fixed by the filter
 * manager and has been 0x80 since Vista. It terminates the operation
 * registration array. */
#ifndef IRP_MJ_OPERATION_END_OF_QUEUE
#define IRP_MJ_OPERATION_END_OF_QUEUE ((UCHAR)0x80)
#endif

/* ExAllocatePoolWithTag is deprecated in favour of ExAllocatePool2, but
 * ExAllocatePool2 does not exist on Windows 7 and this driver targets it so the
 * same binary loads everywhere the project is tested. */
#pragma warning(disable : 4996)

#define SR_TAG 'cRyS'

/* Must match the user-mode side and the shell extension. */
#define SR_PORT_NAME        L"\\SpaceRecycleDeletePort"
#define SR_STREAM_NAME      L":SpaceRecycle.Importance"
#define SR_MAX_PATH_CHARS   520

/* How long the filter waits for a user-mode answer before giving up. A delete
 * runs on the thread the user is waiting on, so the filter must never be able
 * to hang it: on timeout the operation is allowed and the listener is left to
 * report the miss. Failing closed here would turn a crashed service into a
 * machine where nothing can be deleted. */
#define SR_REPLY_TIMEOUT_MS 3000

typedef struct _SR_NOTIFY {
    ULONG Size;
    ULONG Operation;      /* SR_OP_* */
    WCHAR Path[SR_MAX_PATH_CHARS];
} SR_NOTIFY, *PSR_NOTIFY;

typedef struct _SR_REPLY {
    BOOLEAN Allow;
} SR_REPLY, *PSR_REPLY;

#define SR_OP_DELETE 1
#define SR_OP_DELETE_ON_CLOSE 2

static PFLT_FILTER gFilter = NULL;
static PFLT_PORT   gServerPort = NULL;
static PFLT_PORT   gClientPort = NULL;

/* ------------------------------------------------------------- user channel */

static NTSTATUS SRConnect(PFLT_PORT ClientPort, PVOID ServerPortCookie,
                          PVOID ConnectionContext, ULONG SizeOfContext,
                          PVOID* ConnectionPortCookie) {
    UNREFERENCED_PARAMETER(ServerPortCookie);
    UNREFERENCED_PARAMETER(ConnectionContext);
    UNREFERENCED_PARAMETER(SizeOfContext);
    UNREFERENCED_PARAMETER(ConnectionPortCookie);

    /* One listener at a time. A second connection would be another process
     * claiming authority over deletions, and there is no reason to allow it. */
    if (gClientPort != NULL) {
        return STATUS_CONNECTION_COUNT_LIMIT;
    }
    gClientPort = ClientPort;
    return STATUS_SUCCESS;
}

static VOID SRDisconnect(PVOID ConnectionCookie) {
    UNREFERENCED_PARAMETER(ConnectionCookie);
    if (gClientPort != NULL) {
        FltCloseClientPort(gFilter, &gClientPort);
        gClientPort = NULL;
    }
}

static NTSTATUS SRMessage(PVOID ConnectionCookie, PVOID InputBuffer,
                          ULONG InputBufferSize, PVOID OutputBuffer,
                          ULONG OutputBufferSize, PULONG ReturnOutputBufferLength) {
    UNREFERENCED_PARAMETER(ConnectionCookie);
    UNREFERENCED_PARAMETER(InputBuffer);
    UNREFERENCED_PARAMETER(InputBufferSize);
    UNREFERENCED_PARAMETER(OutputBuffer);
    UNREFERENCED_PARAMETER(OutputBufferSize);
    UNREFERENCED_PARAMETER(ReturnOutputBufferLength);
    return STATUS_NOT_IMPLEMENTED;
}

/* Asks user mode whether this path may go. Returns TRUE when the operation
 * should proceed.
 *
 * Every failure path returns TRUE. The filter is a gate, not a lock: if the
 * listener is absent, slow, or returns nonsense, the safe failure for a file
 * system is to stay out of the way. The protection this provides is against a
 * user deleting something by accident, not against a user determined to
 * delete it - the same honesty the subscription gate carries. */
static BOOLEAN SRAskUserMode(_In_ PCUNICODE_STRING Path, ULONG Operation) {
    PSR_NOTIFY notify = NULL;
    SR_REPLY reply;
    ULONG replyLength = 0;
    LARGE_INTEGER timeout;
    NTSTATUS status;

    if (gClientPort == NULL || Path == NULL) {
        return TRUE;
    }
    if (Path->Length / sizeof(WCHAR) >= SR_MAX_PATH_CHARS) {
        return TRUE;    /* too long to describe; do not guess */
    }

    notify = (PSR_NOTIFY)ExAllocatePoolWithTag(NonPagedPool, sizeof(SR_NOTIFY), SR_TAG);
    if (notify == NULL) {
        return TRUE;
    }
    RtlZeroMemory(notify, sizeof(SR_NOTIFY));
    notify->Size = sizeof(SR_NOTIFY);
    notify->Operation = Operation;
    RtlStringCchCopyNW(notify->Path, SR_MAX_PATH_CHARS, Path->Buffer,
                       Path->Length / sizeof(WCHAR));

    RtlZeroMemory(&reply, sizeof(reply));
    /* ReplyLength is an in/out parameter: on the way in it must say how large
     * the reply buffer is, otherwise the kernel believes there is no room for
     * an answer and the user-mode reply fails with ERROR_MORE_DATA. Passing it
     * as 0 makes the policy question unanswerable while looking like it works. */
    replyLength = sizeof(SR_REPLY);
    timeout.QuadPart = -(LONGLONG)SR_REPLY_TIMEOUT_MS * 10000;   /* relative, 100ns */

    status = FltSendMessage(gFilter, &gClientPort, notify, sizeof(SR_NOTIFY),
                            &reply, &replyLength, &timeout);
    ExFreePoolWithTag(notify, SR_TAG);

    if (status == STATUS_SUCCESS && replyLength >= sizeof(SR_REPLY)) {
        return reply.Allow ? TRUE : FALSE;
    }
    if (status == STATUS_TIMEOUT) {
        DbgPrint("SpaceRecycle: listener timed out; allowing\n");
    }
    return TRUE;
}

/* -------------------------------------------------------------- interception */

/* Turns a callback's file object into a normalized DOS path. Returns TRUE and
 * fills Path on success. The caller must call SRReleasePath. */
static BOOLEAN SRGetPath(PFLT_CALLBACK_DATA Data, PFLT_FILE_NAME_INFORMATION* OutInfo) {
    PFLT_FILE_NAME_INFORMATION info = NULL;
    NTSTATUS status;

    *OutInfo = NULL;
    status = FltGetFileNameInformation(
        Data, FLT_FILE_NAME_NORMALIZED | FLT_FILE_NAME_QUERY_DEFAULT, &info);
    if (!NT_SUCCESS(status)) {
        return FALSE;
    }
    status = FltParseFileNameInformation(info);
    if (!NT_SUCCESS(status)) {
        FltReleaseFileNameInformation(info);
        return FALSE;
    }
    *OutInfo = info;
    return TRUE;
}

static VOID SRReleasePath(PFLT_FILE_NAME_INFORMATION Info) {
    if (Info != NULL) {
        FltReleaseFileNameInformation(Info);
    }
}

static FLT_PREOP_CALLBACK_STATUS SRPreSetInformation(
    PFLT_CALLBACK_DATA Data, PCFLT_RELATED_OBJECTS FltObjects, PVOID* CompletionContext) {
    PFLT_FILE_NAME_INFORMATION info = NULL;
    PFILE_DISPOSITION_INFORMATION disposition;
    BOOLEAN deleting = FALSE;
    FLT_PREOP_CALLBACK_STATUS result = FLT_PREOP_SUCCESS_NO_CALLBACK;
    ULONG operation = SR_OP_DELETE;

    UNREFERENCED_PARAMETER(FltObjects);
    UNREFERENCED_PARAMETER(CompletionContext);

    if (Data->Iopb->Parameters.SetFileInformation.FileInformationClass ==
        FileDispositionInformation) {
        disposition = (PFILE_DISPOSITION_INFORMATION)
            Data->Iopb->Parameters.SetFileInformation.InfoBuffer;
        if (disposition != NULL && disposition->DeleteFile) {
            deleting = TRUE;
        }
    } else if (Data->Iopb->Parameters.SetFileInformation.FileInformationClass ==
               FileDispositionInformationEx) {
        /* Windows 10 1709+. Guarded by the class value rather than a build
         * check so the same binary stays loadable on Windows 7, where this
         * class never arrives. */
        PFILE_DISPOSITION_INFORMATION_EX ex =
            (PFILE_DISPOSITION_INFORMATION_EX)
            Data->Iopb->Parameters.SetFileInformation.InfoBuffer;
        if (ex != NULL && FlagOn(ex->Flags, FILE_DISPOSITION_DELETE)) {
            deleting = TRUE;
        }
    }

    if (!deleting) {
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    }

    if (!SRGetPath(Data, &info)) {
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    }

    DbgPrint("SpaceRecycle: delete attempt %wZ\n", &info->Name);

    if (!SRAskUserMode(&info->Name, operation)) {
        Data->IoStatus.Status = STATUS_ACCESS_DENIED;
        Data->IoStatus.Information = 0;
        result = FLT_PREOP_COMPLETE;
    }

    SRReleasePath(info);
    return result;
}

static FLT_PREOP_CALLBACK_STATUS SRPreCreate(
    PFLT_CALLBACK_DATA Data, PCFLT_RELATED_OBJECTS FltObjects, PVOID* CompletionContext) {
    PFLT_FILE_NAME_INFORMATION info = NULL;
    FLT_PREOP_CALLBACK_STATUS result = FLT_PREOP_SUCCESS_NO_CALLBACK;

    UNREFERENCED_PARAMETER(FltObjects);
    UNREFERENCED_PARAMETER(CompletionContext);

    if (!FlagOn(Data->Iopb->Parameters.Create.Options, FILE_DELETE_ON_CLOSE)) {
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    }
    if (!SRGetPath(Data, &info)) {
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    }

    DbgPrint("SpaceRecycle: delete-on-close %wZ\n", &info->Name);

    if (!SRAskUserMode(&info->Name, SR_OP_DELETE_ON_CLOSE)) {
        Data->IoStatus.Status = STATUS_ACCESS_DENIED;
        Data->IoStatus.Information = 0;
        result = FLT_PREOP_COMPLETE;
    }

    SRReleasePath(info);
    return result;
}

static const FLT_OPERATION_REGISTRATION SRCallbacks[] = {
    { IRP_MJ_SET_INFORMATION, 0, SRPreSetInformation, NULL },
    { IRP_MJ_CREATE, 0, SRPreCreate, NULL },
    { IRP_MJ_OPERATION_END_OF_QUEUE, 0, NULL, NULL }
};

/* Without this the filter can never be unloaded - the registration structure
 * documents a NULL unload callback as meaning exactly that - and the first
 * attempt to swap in a new build failed with STATUS_FLT_DO_NOT_DETACH
 * (0x801f0010). A filter that needs a reboot to update or remove is a filter
 * nobody can service.
 *
 * The filter manager performs the deregistration itself; this only has to let
 * go of what the driver owns. */
static NTSTATUS SRUnload(FLT_FILTER_UNLOAD_FLAGS Flags) {
    UNREFERENCED_PARAMETER(Flags);
    if (gServerPort != NULL) {
        FltCloseCommunicationPort(gServerPort);
        gServerPort = NULL;
    }
    return STATUS_SUCCESS;
}

/* Filled in by field name in DriverEntry rather than positionally: the
 * structure has grown across WDK versions, and a positional initialiser that
 * silently drifts by one field would register the wrong callbacks. */
static FLT_REGISTRATION gRegistration;

/* -------------------------------------------------------------------- setup */

NTSTATUS DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath) {
    NTSTATUS status;
    UNICODE_STRING portName;
    OBJECT_ATTRIBUTES attributes;
    PSECURITY_DESCRIPTOR sd = NULL;

    UNREFERENCED_PARAMETER(RegistryPath);

    RtlZeroMemory(&gRegistration, sizeof(gRegistration));
    gRegistration.Size = sizeof(FLT_REGISTRATION);
    gRegistration.Version = FLT_REGISTRATION_VERSION;
    gRegistration.OperationRegistration = SRCallbacks;
    gRegistration.FilterUnloadCallback = SRUnload;

    status = FltRegisterFilter(DriverObject, &gRegistration, &gFilter);
    if (!NT_SUCCESS(status)) {
        DbgPrint("SpaceRecycle: FltRegisterFilter failed 0x%08X\n", status);
        return status;
    }

    /* The port is how the policy question reaches user mode. Its security
     * descriptor is left NULL here, so the filter manager applies the default
     * for the account the service runs under; a product build should restrict
     * it to the service's SID and to administrators. */
    RtlInitUnicodeString(&portName, SR_PORT_NAME);
    InitializeObjectAttributes(&attributes, &portName,
                               OBJ_KERNEL_HANDLE | OBJ_CASE_INSENSITIVE, NULL, sd);

    status = FltCreateCommunicationPort(gFilter, &gServerPort, &attributes, NULL,
                                        SRConnect, SRDisconnect, SRMessage, 1);
    if (!NT_SUCCESS(status)) {
        DbgPrint("SpaceRecycle: FltCreateCommunicationPort failed 0x%08X\n", status);
        FltUnregisterFilter(gFilter);
        gFilter = NULL;
        return status;
    }

    status = FltStartFiltering(gFilter);
    if (!NT_SUCCESS(status)) {
        DbgPrint("SpaceRecycle: FltStartFiltering failed 0x%08X\n", status);
        FltCloseCommunicationPort(gServerPort);
        FltUnregisterFilter(gFilter);
        gFilter = NULL;
        return status;
    }

    DbgPrint("SpaceRecycle: filter started\n");
    return STATUS_SUCCESS;
}