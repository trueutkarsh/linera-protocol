// Copyright (c) Zefchain Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Abstraction over how different runtimes manipulate the guest WebAssembly module's memory.

use super::{InstanceWithFunction, Runtime, RuntimeError};
use crate::{Layout, WitType};
use frunk::{hlist, hlist_pat, HList};
use std::borrow::Cow;

/// An address for a location in a guest WebAssembly module's memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestPointer(u32);

impl GuestPointer {
    /// Returns a new address that's the current address advanced to after the size of `T`.
    pub fn after<T: WitType>(&self) -> Self {
        GuestPointer(self.0 + T::SIZE)
    }

    /// Returns a new address that's the current address advanced to add padding to ensure it's
    /// aligned properly for `T`.
    pub fn after_padding_for<T: WitType>(&self) -> Self {
        let padding = (-(self.0 as i32) & (<T::Layout as Layout>::ALIGNMENT as i32 - 1)) as u32;

        GuestPointer(self.0 + padding)
    }

    /// Returns the address of an element in a contiguous list of properly aligned `T` types.
    pub fn index<T: WitType>(&self, index: u32) -> Self {
        let element_size = GuestPointer(T::SIZE).after_padding_for::<T>();

        GuestPointer(self.0 + index * element_size.0)
    }
}

/// Interface for accessing a runtime specific memory.
pub trait RuntimeMemory<Instance> {
    /// Reads `length` bytes from memory from the provided `location`.
    fn read<'instance>(
        &self,
        instance: &'instance Instance,
        location: GuestPointer,
        length: u32,
    ) -> Result<Cow<'instance, [u8]>, RuntimeError>;

    /// Writes the `bytes` to memory at the provided `location`.
    fn write(
        &mut self,
        instance: &mut Instance,
        location: GuestPointer,
        bytes: &[u8],
    ) -> Result<(), RuntimeError>;
}

/// Trait alias for a Wasm module instance with the WIT Canonical ABI `cabi_realloc` function.
pub trait CabiReallocAlias: InstanceWithFunction<HList![i32, i32, i32, i32], HList![i32]> {}

impl<AnyInstance> CabiReallocAlias for AnyInstance where
    AnyInstance: InstanceWithFunction<HList![i32, i32, i32, i32], HList![i32]>
{
}

/// Trait alias for a Wasm module instance with the WIT Canonical ABI `cabi_free` function.
pub trait CabiFreeAlias: InstanceWithFunction<HList![i32], HList![]> {}

impl<AnyInstance> CabiFreeAlias for AnyInstance where
    AnyInstance: InstanceWithFunction<HList![i32], HList![]>
{
}

/// A handle to interface with a guest Wasm module instance's memory.
#[allow(clippy::type_complexity)]
pub struct Memory<'runtime, Instance>
where
    Instance: CabiReallocAlias + CabiFreeAlias,
{
    instance: &'runtime mut Instance,
    memory: <Instance::Runtime as Runtime>::Memory,
    cabi_realloc: Option<
        <Instance as InstanceWithFunction<HList![i32, i32, i32, i32], HList![i32]>>::Function,
    >,
    cabi_free: Option<<Instance as InstanceWithFunction<HList![i32], HList![]>>::Function>,
}

impl<Instance> Memory<'_, Instance>
where
    Instance: CabiReallocAlias + CabiFreeAlias,
    <Instance::Runtime as Runtime>::Memory: RuntimeMemory<Instance>,
{
    /// Reads `length` bytes from `location`.
    ///
    /// The underlying runtime may return either a memory slice or an owned buffer.
    pub fn read(&self, location: GuestPointer, length: u32) -> Result<Cow<'_, [u8]>, RuntimeError> {
        self.memory.read(&*self.instance, location, length)
    }

    /// Writes `bytes` to `location`.
    pub fn write(&mut self, location: GuestPointer, bytes: &[u8]) -> Result<(), RuntimeError> {
        self.memory.write(&mut *self.instance, location, bytes)
    }

    /// Returns a newly allocated buffer of `size` bytes in the guest module's memory.
    ///
    /// Calls the guest module to allocate the memory, so the resulting allocation is managed by
    /// the guest.
    pub fn allocate(&mut self, size: u32) -> Result<GuestPointer, RuntimeError> {
        if self.cabi_realloc.is_none() {
            self.cabi_realloc = Some(<Instance as InstanceWithFunction<
                HList![i32, i32, i32, i32],
                HList![i32],
            >>::load_function(self.instance, "cabi_realloc")?);
        }

        let size = i32::try_from(size).map_err(|_| RuntimeError::AllocationTooLarge)?;

        let cabi_realloc = self
            .cabi_realloc
            .as_ref()
            .expect("`cabi_realloc` function was not loaded before it was called");

        let hlist_pat![allocation_address] =
            self.instance.call(cabi_realloc, hlist![0, 0, 1, size])?;

        Ok(GuestPointer(
            allocation_address
                .try_into()
                .map_err(|_| RuntimeError::AllocationFailed)?,
        ))
    }

    /// Deallocates the `allocation` managed by the guest.
    pub fn deallocate(&mut self, allocation: GuestPointer) -> Result<(), RuntimeError> {
        if self.cabi_free.is_none() {
            self.cabi_free = Some(
                <Instance as InstanceWithFunction<HList![i32], HList![]>>::load_function(
                    self.instance,
                    "cabi_free",
                )?,
            );
        }

        let address = allocation
            .0
            .try_into()
            .map_err(|_| RuntimeError::DeallocateInvalidAddress)?;

        let cabi_free = self
            .cabi_free
            .as_ref()
            .expect("`cabi_free` function was not loaded before it was called");

        self.instance.call(cabi_free, hlist![address])?;

        Ok(())
    }
}
