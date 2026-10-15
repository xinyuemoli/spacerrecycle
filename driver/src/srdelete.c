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

/* What to do about a delete. The driver is told this; it never derives it.
 *
 * Every *value* lives in user mode, so a settings screen can change the
 * disposition without the driver being rebuilt or reloaded, and without the
 * service being restarted. Only a new *kind* of action - something the driver
 * cannot already express by allowing, refusing or asking - would need driver
 * code. */
#define SR_ACTION_ASK   0   /* escalate to user mode and let it decide */
#define SR_ACTION_DENY  1   /* refuse here: no message, no round trip */
#define SR_ACTION_ALLOW 2   /* permit here: no message, no round trip */

#define SR_CMD_SET_POLICY 1
#define SR_CMD_GET_POLICY 2

/* Which job a client connection is for.
 *
 * Two connections exist because one port handle cannot carry both directions at
 * once: a FilterSendMessage issued while the other side is blocked in
 * FilterGetMessage on the same handle corrupts the receive, which returns
 * success without writing the buffer. The request that was waiting is then
 * answered by nobody, the three-second timeout expires, and the delete fails
 * open - so a policy change could cost a delete its protection. Separate
 * handles mean the two directions never share a queue. */
#define SR_PORT_ROLE_REQUESTS 1
#define SR_PORT_ROLE_POLICY   2
#define SR_MAX_CONNECTIONS    2

typedef struct _SR_POLICY {
    ULONG Size;               /* sizeof(SR_POLICY). A newer user-mode side
                               * sending a bigger structure is rejected rather
                               * than having its extra fields read as garbage. */
    ULONG Version;
    ULONG ActionForMarked;    /* SR_ACTION_* */
    ULONG ActionForUnmarked;  /* SR_ACTION_* */
    ULONG CheckMark;          /* 1 = test the ADS before escalating */
} SR_POLICY, *PSR_POLICY;

/* Echoed back with every policy push.
 *
 * The counters exist because a driver has no console: without them, "the check
 * says unmarked" and "the check is never reached" and "the pushed policy says
 * allow" all look identical from user mode - a delete that simply succeeds.
 * Returning them alongside the applied policy makes the whole decision path
 * observable in one call, which is what turned a silent misbehaviour into a
 * specific bug twice during development. */
typedef struct _SR_STATUS {
    SR_POLICY Policy;
    ULONG MarkChecked;
    ULONG MarkYes;
    ULONG MarkNo;
    ULONG Decisions;
    ULONG Escalated;
    ULONG LastMarkStatus;   /* NTSTATUS from the last stream open */
    ULONG Connects;         /* times a client attached */
    ULONG Disconnects;      /* times a client detached */
    ULONG PortReplaced;     /* times a new client displaced an old one */
    ULONG HasRequestPort;   /* 1 when a delete-request client is attached */
    ULONG HasPolicyPort;    /* 1 when a policy client is attached */
} SR_STATUS, *PSR_STATUS;

static PFLT_FILTER gFilter = NULL;
static PFLT_PORT   gServerPort = NULL;
/* The client that receives delete requests. FltSendMessage sends here, so the
 * service must keep a thread blocked on this handle's FilterGetMessage. */
static PFLT_PORT   gClientPort = NULL;
/* The client used for policy pushes. Nothing ever blocks on it, which is the
 * whole point of keeping it separate. */
static PFLT_PORT   gPolicyPort = NULL;

/* The live policy, and the safe default: look at the mark, escalate anything
 * marked, and stay completely out of the way for everything else. That is what
 * the driver does before a service has said otherwise, and what it falls back
 * to if a push never arrives. */
static SR_POLICY gPolicy = {
    sizeof(SR_POLICY),
    1,
    SR_ACTION_ASK,
    SR_ACTION_ALLOW,
    1
};

static volatile LONG gMarkChecked = 0;
static volatile LONG gMarkYes = 0;
static volatile LONG gMarkNo = 0;
static volatile LONG gDecisions = 0;
static volatile LONG gEscalated = 0;
static volatile LONG gLastMarkStatus = 0;
static volatile LONG gConnects = 0;
static volatile LONG gDisconnects = 0;
static volatile LONG gPortReplaced = 0;

/* ------------------------------------------------------------- user channel */

static NTSTATUS SRConnect(PFLT_PORT ClientPort, PVOID ServerPortCookie,
                          PVOID ConnectionContext, ULONG SizeOfContext,
                          PVOID* ConnectionPortCookie) {
    ULONG role = SR_PORT_ROLE_REQUESTS;

    UNREFERENCED_PARAMETER(ServerPortCookie);

    /* The caller states which job it is connecting for. Anything unrecognised
     * is treated as a request client, because that is the connection the
     * protection depends on. */
    if (ConnectionContext != NULL && SizeOfContext >= sizeof(ULONG)) {
        ULONG requested = *(PULONG)ConnectionContext;
        if (requested == SR_PORT_ROLE_POLICY) {
            role = SR_PORT_ROLE_POLICY;
        }
    }

    /* A new connection replaces the old one of the same role rather than being
     * refused.
     *
     * The first version returned STATUS_CONNECTION_COUNT_LIMIT while any client
     * was attached. That looked like a sensible guard and was in fact a way for
     * the protection to disappear: when the guard service was killed and
     * restarted, its client port had not been released, so the replacement
     * could not connect at all and got ERROR_CONNECTION_COUNT_LIMIT
     * (0x800704D6). With no client attached, every delete then failed open, and
     * fail-open is silent by design.
     *
     * Replacement is per role, so a fresh policy connection does not evict the
     * client that answers deletes. */
    if (role == SR_PORT_ROLE_POLICY) {
        if (gPolicyPort != NULL) {
            InterlockedIncrement(&gPortReplaced);
            FltCloseClientPort(gFilter, &gPolicyPort);
            gPolicyPort = NULL;
        }
        gPolicyPort = ClientPort;
    } else {
        if (gClientPort != NULL) {
            InterlockedIncrement(&gPortReplaced);
            FltCloseClientPort(gFilter, &gClientPort);
            gClientPort = NULL;
        }
        gClientPort = ClientPort;
    }

    /* Handed back on disconnect so the right slot is cleared. Without it, a
     * policy client going away would take the request port with it. */
    if (ConnectionPortCookie != NULL) {
        *ConnectionPortCookie = (PVOID)(ULONG_PTR)role;
    }
    InterlockedIncrement(&gConnects);
    return STATUS_SUCCESS;
}

static VOID SRDisconnect(PVOID ConnectionCookie) {
    ULONG role = (ULONG)(ULONG_PTR)ConnectionCookie;

    InterlockedIncrement(&gDisconnects);
    if (role == SR_PORT_ROLE_POLICY) {
        if (gPolicyPort != NULL) {
            FltCloseClientPort(gFilter, &gPolicyPort);
            gPolicyPort = NULL;
        }
    } else {
        if (gClientPort != NULL) {
            FltCloseClientPort(gFilter, &gClientPort);
            gClientPort = NULL;
        }
    }
}

/* Policy push. This is the user-mode -> driver direction: the service calls
 * FilterSendMessage, which lands here. It is what makes the disposition
 * changeable at runtime - the alternative would be a registry value read at
 * load time, which is a reboot to change, or a hard-coded constant, which is a
 * rebuild. */
static void SRFillStatus(PSR_STATUS Status) {
    Status->Policy = gPolicy;
    Status->MarkChecked = (ULONG)gMarkChecked;
    Status->MarkYes = (ULONG)gMarkYes;
    Status->MarkNo = (ULONG)gMarkNo;
    Status->Decisions = (ULONG)gDecisions;
    Status->Escalated = (ULONG)gEscalated;
    Status->LastMarkStatus = (ULONG)gLastMarkStatus;
    Status->Connects = (ULONG)gConnects;
    Status->Disconnects = (ULONG)gDisconnects;
    Status->PortReplaced = (ULONG)gPortReplaced;
    Status->HasRequestPort = (gClientPort != NULL) ? 1 : 0;
    Status->HasPolicyPort = (gPolicyPort != NULL) ? 1 : 0;
}

static NTSTATUS SRMessage(PVOID ConnectionCookie, PVOID InputBuffer,
                          ULONG InputBufferSize, PVOID OutputBuffer,
                          ULONG OutputBufferSize, PULONG ReturnOutputBufferLength) {
    ULONG command;

    UNREFERENCED_PARAMETER(ConnectionCookie);

    if (ReturnOutputBufferLength != NULL) {
        *ReturnOutputBufferLength = 0;
    }
    if (InputBuffer == NULL || InputBufferSize < sizeof(ULONG)) {
        return STATUS_INVALID_PARAMETER;
    }

    command = *(PULONG)InputBuffer;

    if (command == SR_CMD_SET_POLICY) {
        PSR_POLICY incoming;
        /* The buffer starts with the command word; the structure follows it.
         * Reading it from offset 0 makes Size and Version come out as the
         * command value, which the validation below then rejects with
         * STATUS_REVISION_MISMATCH - a push that fails while looking like a
         * version problem. */
        if (InputBufferSize < sizeof(ULONG) + sizeof(SR_POLICY)) {
            return STATUS_INVALID_PARAMETER;
        }
        incoming = (PSR_POLICY)((PUCHAR)InputBuffer + sizeof(ULONG));
        /* Refuse a structure this build does not understand, and any action
         * value that is not one we implement: silently storing an unknown
         * action would leave the driver unable to decide and it would fall
         * through to allowing deletes. */
        if (incoming->Size != sizeof(SR_POLICY) || incoming->Version != 1) {
            return STATUS_REVISION_MISMATCH;
        }
        if (incoming->ActionForMarked > SR_ACTION_ALLOW ||
            incoming->ActionForUnmarked > SR_ACTION_ALLOW) {
            return STATUS_INVALID_PARAMETER;
        }

        gPolicy.ActionForMarked = incoming->ActionForMarked;
        gPolicy.ActionForUnmarked = incoming->ActionForUnmarked;
        gPolicy.CheckMark = incoming->CheckMark ? 1 : 0;
        DbgPrint("SpaceRecycle: policy set marked=%lu unmarked=%lu check=%lu\n",
                 gPolicy.ActionForMarked, gPolicy.ActionForUnmarked, gPolicy.CheckMark);
    } else if (command != SR_CMD_GET_POLICY) {
        return STATUS_NOT_IMPLEMENTED;
    }

    /* Both commands answer with the same structure, and the answer is written
     * on the way out of a push rather than requiring a second call. That is not
     * only fewer round trips: reading the policy back in a separate
     * FilterSendMessage returned ERROR_INVALID_HANDLE here, and a verification
     * step that cannot run is worse than no verification step. */
    if (OutputBuffer != NULL && OutputBufferSize >= sizeof(SR_STATUS)) {
        SRFillStatus((PSR_STATUS)OutputBuffer);
        if (ReturnOutputBufferLength != NULL) {
            *ReturnOutputBufferLength = sizeof(SR_STATUS);
        }
    }
    return STATUS_SUCCESS;
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

/* ------------------------------------------------------------------ policy */

#define SR_STREAM_NAME_CHARS ((sizeof(SR_STREAM_NAME) / sizeof(WCHAR)) - 1)

/* Does this file carry the mark at all?
 *
 * This is the whole reason the driver is not chatty. A file with no
 * SpaceRecycle.Importance stream is not something this product has an opinion
 * about, and the answer costs one failed open against a name that is not in the
 * file's own record - no message, no user-mode thread, no context switch. On a
 * machine doing thousands of deletes an hour, that is the difference between a
 * filter nobody notices and one that shows up in every profile.
 *
 * The failure handling is deliberately asymmetric. Only "no such name" means
 * unmarked. Any other error - access denied, a volume that has gone away -
 * escalates, because a mark that cannot be read is not the same as a mark that
 * is not there, and treating the two alike is a silent hole in the protection. */
static BOOLEAN SRHasMark(PCFLT_RELATED_OBJECTS FltObjects, PCUNICODE_STRING Path) {
    WCHAR buffer[SR_MAX_PATH_CHARS + 64];
    UNICODE_STRING streamName;
    OBJECT_ATTRIBUTES attributes;
    IO_STATUS_BLOCK iosb;
    HANDLE handle = NULL;
    NTSTATUS status;
    USHORT chars;

    InterlockedIncrement(&gMarkChecked);

    if (Path == NULL || Path->Buffer == NULL) {
        InterlockedIncrement(&gMarkYes);
        return TRUE;
    }
    chars = Path->Length / sizeof(WCHAR);
    if (chars + SR_STREAM_NAME_CHARS >= (USHORT)(sizeof(buffer) / sizeof(WCHAR))) {
        InterlockedIncrement(&gMarkYes);
        return TRUE;    /* too long to describe; do not guess */
    }

    RtlCopyMemory(buffer, Path->Buffer, Path->Length);
    RtlCopyMemory(buffer + chars, SR_STREAM_NAME,
                  (SR_STREAM_NAME_CHARS + 1) * sizeof(WCHAR));

    RtlInitUnicodeString(&streamName, buffer);
    InitializeObjectAttributes(&attributes, &streamName,
                               OBJ_KERNEL_HANDLE | OBJ_CASE_INSENSITIVE, NULL, NULL);

    /* The stream is a plain data stream of the file being deleted, so a normal
     * open on it answers the question. Note this open comes back through our
     * own IRP_MJ_CREATE handler; it carries no FILE_DELETE_ON_CLOSE, so that
     * handler returns immediately and there is no recursion. */
    status = FltCreateFile(gFilter, FltObjects->Instance, &handle,
                           FILE_READ_ATTRIBUTES | SYNCHRONIZE, &attributes, &iosb,
                           NULL, 0,
                           FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                           FILE_OPEN,
                           FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT,
                           NULL, 0, 0);
    if (NT_SUCCESS(status)) {
        FltClose(handle);
        InterlockedExchange(&gLastMarkStatus, (LONG)status);
        InterlockedIncrement(&gMarkYes);
        return TRUE;
    }
    InterlockedExchange(&gLastMarkStatus, (LONG)status);
    if (status == STATUS_OBJECT_NAME_NOT_FOUND || status == STATUS_OBJECT_PATH_NOT_FOUND) {
        InterlockedIncrement(&gMarkNo);
        return FALSE;
    }
    InterlockedIncrement(&gMarkYes);
    return TRUE;        /* could not tell; escalate rather than assume */
}

/* The single place a delete decision is made. Returns TRUE to allow.
 *
 * Order is the point. The mark check runs first, so a file nobody has marked
 * never costs a message. The policy then decides whether the marked case is
 * answered here or passed up - which is what makes "do not prompt, just refuse"
 * a settings-screen change rather than a driver change. */
static BOOLEAN SRDecide(PCFLT_RELATED_OBJECTS FltObjects,
                        PFLT_FILE_NAME_INFORMATION Info, ULONG Operation) {
    BOOLEAN marked = gPolicy.CheckMark ? SRHasMark(FltObjects, &Info->Name) : TRUE;
    ULONG action = marked ? gPolicy.ActionForMarked : gPolicy.ActionForUnmarked;

    InterlockedIncrement(&gDecisions);

    if (action == SR_ACTION_DENY) {
        return FALSE;
    }
    if (action == SR_ACTION_ALLOW) {
        return TRUE;
    }
    InterlockedIncrement(&gEscalated);
    return SRAskUserMode(&Info->Name, Operation);
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

    if (!SRDecide(FltObjects, info, operation)) {
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

    UNREFERENCED_PARAMETER(CompletionContext);

    if (!FlagOn(Data->Iopb->Parameters.Create.Options, FILE_DELETE_ON_CLOSE)) {
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    }
    if (!SRGetPath(Data, &info)) {
        return FLT_PREOP_SUCCESS_NO_CALLBACK;
    }

    DbgPrint("SpaceRecycle: delete-on-close %wZ\n", &info->Name);

    if (!SRDecide(FltObjects, info, SR_OP_DELETE_ON_CLOSE)) {
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
                                        SRConnect, SRDisconnect, SRMessage,
                                        SR_MAX_CONNECTIONS);
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