//! Win32 security plumbing: a `SECURITY_ATTRIBUTES` with a DACL that grants
//! the current user full access to the named pipe — and nobody else.

use windows::Win32::Foundation::HLOCAL;
use windows::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SetEntriesInAclW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows::Win32::Security::{
    ACL, InitializeSecurityDescriptor, NO_INHERITANCE, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    SECURITY_DESCRIPTOR, SetSecurityDescriptorDacl,
};
use windows::Win32::Storage::FileSystem::{FILE_GENERIC_READ, FILE_GENERIC_WRITE};
use windows::core::PWSTR;

/// Owns the SID buffer, the DACL, and the security descriptor referenced by
/// the exposed `SECURITY_ATTRIBUTES`. Keep it alive across the
/// `CreateNamedPipeW` call.
pub struct UserOnlySecurity {
    /// The attributes passed to `CreateNamedPipeW`.
    pub attrs: SECURITY_ATTRIBUTES,
    // The fields below are never *read*: they exist to own the raw buffers the
    // attributes point into, and freeing them safely in `Drop` (the mandatory
    // pairing for `SetEntriesInAclW`/`LocalAlloc` and our leaked SID `Box`).
    #[allow(dead_code)]
    acl: *mut ACL,
    #[allow(dead_code)]
    descriptor: Box<[u8; size_of::<SECURITY_DESCRIPTOR>()]>,
    #[allow(dead_code)]
    sid: *mut u8,
    #[allow(dead_code)]
    sid_len: usize,
    /// Phantom keeps this struct `!Send`: the Win32 pointers are thread-local.
    _local: std::marker::PhantomData<*const ()>,
}

/// Builds a `SECURITY_ATTRIBUTES` whose DACL contains a single allow-ACE for
/// the current user (read + write on the pipe object).
///
/// `sid` is the raw SID bytes from the process token. When `None` or on any
/// failure the function returns `None` and the caller falls back to the
/// default DACL (still safe in a single-user session, logged in a follow-up).
#[must_use]
pub fn user_only_security_attributes(sid: Option<&[u8]>) -> Option<UserOnlySecurity> {
    let sid = sid?;
    if sid.len() < 8 || sid.first() != Some(&1) {
        return None; // SIDs start with revision 1.
    }
    // Stable SID storage: a leaked Box we reclaim in `Drop`.
    let mut sid_box: Box<[u8]> = sid.to_vec().into_boxed_slice();
    let sid_ptr = sid_box.as_mut_ptr();
    let sid_ps = PSID(sid_ptr.cast());

    let trustee = TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_USER,
        // SAFETY (field doc): `ptstrName` borrows the SID buffer owned by
        // `UserOnlySecurity` for the whole lifetime of the attributes. Typed
        // as PWSTR because Win32 unions the form; `TRUSTEE_IS_SID` selects PSID.
        ptstrName: PWSTR(sid_ps.0.cast::<u16>()),
    };
    let entry = EXPLICIT_ACCESS_W {
        // Read + write on the pipe: exactly what the protocol needs.
        grfAccessPermissions: (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
        grfAccessMode: GRANT_ACCESS,
        // Inheritance does not apply to pipes.
        grfInheritance: NO_INHERITANCE,
        Trustee: trustee,
    };

    let mut acl: *mut ACL = std::ptr::null_mut();
    // SAFETY: `entry` is a valid single-element slice; `acl` receives an
    // ACL allocated with `LocalAlloc` and freed in `Drop`.
    let status = unsafe { SetEntriesInAclW(Some(std::slice::from_ref(&entry)), None, &raw mut acl) };
    if status.0 != 0 || acl.is_null() {
        drop(sid_box);
        return None;
    }

    let mut descriptor: Box<[u8; size_of::<SECURITY_DESCRIPTOR>()]> = Box::new([0u8; size_of::<SECURITY_DESCRIPTOR>()]);
    let desc = PSECURITY_DESCRIPTOR(descriptor.as_mut_ptr().cast());
    // SAFETY: `descriptor` provides `size_of::<SECURITY_DESCRIPTOR>()` writable
    // bytes — the documented requirement for `InitializeSecurityDescriptor`.
    if unsafe { InitializeSecurityDescriptor(desc, 1) }.is_err() {
        // SAFETY: `acl` was allocated by `SetEntriesInAclW` (`LocalAlloc`).
        unsafe {
            dealloc_acl(acl);
        };
        drop(descriptor);
        drop(sid_box);
        return None;
    }
    // SAFETY: `desc` is an absolute-format descriptor; `acl` is a valid DACL
    // built by `SetEntriesInAclW`, not defaulted.
    if unsafe { SetSecurityDescriptorDacl(desc, true, Some(acl), false) }.is_err() {
        // SAFETY: as above.
        unsafe {
            dealloc_acl(acl);
        };
        drop(descriptor);
        drop(sid_box);
        return None;
    }

    let sid_len = sid_box.len();
    std::mem::forget(sid_box); // reclaimed via `sid_ptr` in `Drop`.

    Some(UserOnlySecurity {
        attrs: SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            // SAFETY (field doc): `lpSecurityDescriptor` points into
            // `descriptor`, owned by this struct and outliving the
            // `SECURITY_ATTRIBUTES` borrow.
            lpSecurityDescriptor: desc.0,
            bInheritHandle: false.into(),
        },
        acl,
        descriptor,
        sid: sid_ptr,
        sid_len,
        _local: std::marker::PhantomData,
    })
}

/// Frees a `LocalAlloc`-backed ACL from `SetEntriesInAclW`.
///
/// # Safety
/// `acl` must have been allocated by `SetEntriesInAclW` and not freed yet.
unsafe fn dealloc_acl(acl: *mut ACL) {
    // SAFETY: see Safety on fn; the return value is the freed block handle.
    let _ = unsafe { windows::Win32::Foundation::LocalFree(Some(HLOCAL(acl.cast()))) };
}
