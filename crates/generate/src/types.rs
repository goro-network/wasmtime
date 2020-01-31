use crate::names::Names;

use proc_macro2::TokenStream;
use quote::quote;
use witx::Layout;

pub fn define_datatype(names: &Names, namedtype: &witx::NamedType) -> TokenStream {
    match &namedtype.tref {
        witx::TypeRef::Name(alias_to) => define_alias(names, &namedtype.name, &alias_to),
        witx::TypeRef::Value(v) => match &**v {
            witx::Type::Enum(e) => define_enum(names, &namedtype.name, &e),
            witx::Type::Int(_) => unimplemented!("int types"),
            witx::Type::Flags(_) => unimplemented!("flag types"),
            witx::Type::Struct(s) => {
                if struct_is_copy(s) {
                    define_copy_struct(names, &namedtype.name, &s)
                } else {
                    define_ptr_struct(names, &namedtype.name, &s)
                }
            }
            witx::Type::Union(_) => unimplemented!("union types"),
            witx::Type::Handle(_h) => unimplemented!("handle types"),
            witx::Type::Builtin(b) => define_builtin(names, &namedtype.name, *b),
            witx::Type::Pointer(p) => {
                define_witx_pointer(names, &namedtype.name, quote!(::memory::GuestPtrMut), p)
            }
            witx::Type::ConstPointer(p) => {
                define_witx_pointer(names, &namedtype.name, quote!(::memory::GuestPtr), p)
            }
            witx::Type::Array { .. } => unimplemented!("array types"),
        },
    }
}

fn define_alias(names: &Names, name: &witx::Id, to: &witx::NamedType) -> TokenStream {
    let ident = names.type_(name);
    let rhs = names.type_(&to.name);
    if type_needs_lifetime(&to.tref) {
        quote!(pub type #ident<'a> = #rhs<'a>;)
    } else {
        quote!(pub type #ident = #rhs;)
    }
}

fn define_enum(names: &Names, name: &witx::Id, e: &witx::EnumDatatype) -> TokenStream {
    let ident = names.type_(&name);

    let repr = int_repr_tokens(e.repr);
    let abi_repr = atom_token(match e.repr {
        witx::IntRepr::U8 | witx::IntRepr::U16 | witx::IntRepr::U32 => witx::AtomType::I32,
        witx::IntRepr::U64 => witx::AtomType::I64,
    });

    let variant_names = e.variants.iter().map(|v| names.enum_variant(&v.name));
    let tryfrom_repr_cases = e.variants.iter().enumerate().map(|(n, v)| {
        let variant_name = names.enum_variant(&v.name);
        quote!(#n => Ok(#ident::#variant_name))
    });
    let to_repr_cases = e.variants.iter().enumerate().map(|(n, v)| {
        let variant_name = names.enum_variant(&v.name);
        quote!(#ident::#variant_name => #n as #repr)
    });

    quote! {
        #[repr(#repr)]
        #[derive(Copy, Clone, Debug, ::std::hash::Hash, Eq, PartialEq)]
        pub enum #ident {
            #(#variant_names),*
        }

        impl ::std::convert::TryFrom<#repr> for #ident {
            type Error = ::memory::GuestError;
            fn try_from(value: #repr) -> Result<#ident, ::memory::GuestError> {
                match value as usize {
                    #(#tryfrom_repr_cases),*,
                    _ => Err(::memory::GuestError::InvalidEnumValue(stringify!(#ident))),
                }
            }
        }

        impl ::std::convert::TryFrom<#abi_repr> for #ident {
            type Error = ::memory::GuestError;
            fn try_from(value: #abi_repr) -> Result<#ident, ::memory::GuestError> {
                #ident::try_from(value as #repr)
            }
        }

        impl From<#ident> for #repr {
            fn from(e: #ident) -> #repr {
                match e {
                    #(#to_repr_cases),*
                }
            }
        }

        impl From<#ident> for #abi_repr {
            fn from(e: #ident) -> #abi_repr {
                #repr::from(e) as #abi_repr
            }
        }

        impl ::memory::GuestType for #ident {
            fn size() -> u32 {
                ::std::mem::size_of::<#repr>() as u32
            }
            fn align() -> u32 {
                ::std::mem::align_of::<#repr>() as u32
            }
            fn name() -> String {
                stringify!(#ident).to_owned()
            }
            fn validate<'a>(location: &::memory::GuestPtr<'a, #ident>) -> Result<(), ::memory::GuestError> {
                use ::std::convert::TryFrom;
                let raw: #repr = unsafe { (location.as_raw() as *const #repr).read() };
                let _ = #ident::try_from(raw)?;
                Ok(())
            }
        }

        impl ::memory::GuestTypeCopy for #ident {}
        impl ::memory::GuestTypeClone for #ident {
            fn read_from_guest(location: &::memory::GuestPtr<#ident>) -> Result<#ident, ::memory::GuestError> {
                use ::std::convert::TryFrom;
                let raw: #repr = unsafe { (location.as_raw() as *const #repr).read() };
                let val = #ident::try_from(raw)?;
                Ok(val)
            }
            fn write_to_guest(&self, location: &::memory::GuestPtrMut<#ident>) {
                let val: #repr = #repr::from(*self);
                unsafe { (location.as_raw() as *mut #repr).write(val) };
            }
        }

    }
}

fn define_builtin(names: &Names, name: &witx::Id, builtin: witx::BuiltinType) -> TokenStream {
    let ident = names.type_(name);
    let built = names.builtin_type(builtin);
    quote!(pub type #ident = #built;)
}

pub fn type_needs_lifetime(tref: &witx::TypeRef) -> bool {
    match &*tref.type_() {
        witx::Type::Builtin(b) => match b {
            witx::BuiltinType::String => unimplemented!(),
            _ => false,
        },
        witx::Type::Enum { .. }
        | witx::Type::Flags { .. }
        | witx::Type::Int { .. }
        | witx::Type::Handle { .. } => false,
        witx::Type::Struct(s) => !struct_is_copy(&s),
        witx::Type::Union { .. } => true,
        witx::Type::Pointer { .. } | witx::Type::ConstPointer { .. } => true,
        witx::Type::Array { .. } => unimplemented!(),
    }
}

pub fn struct_is_copy(s: &witx::StructDatatype) -> bool {
    s.members.iter().all(|m| match &*m.tref.type_() {
        witx::Type::Struct(s) => struct_is_copy(&s),
        witx::Type::Builtin(b) => match &*b {
            witx::BuiltinType::String => false,
            _ => true,
        },
        witx::Type::ConstPointer { .. }
        | witx::Type::Pointer { .. }
        | witx::Type::Array { .. }
        | witx::Type::Union { .. } => false,
        witx::Type::Enum { .. }
        | witx::Type::Int { .. }
        | witx::Type::Flags { .. }
        | witx::Type::Handle { .. } => true,
    })
}

fn define_copy_struct(names: &Names, name: &witx::Id, s: &witx::StructDatatype) -> TokenStream {
    let ident = names.type_(name);
    let size = s.mem_size_align().size as u32;
    let align = s.mem_size_align().align as u32;

    let member_decls = s.members.iter().map(|m| {
        let name = names.struct_member(&m.name);
        let type_ = names.type_ref(&m.tref, anon_lifetime());
        quote!(pub #name: #type_)
    });
    let member_valids = s.member_layout().into_iter().map(|ml| {
        let type_ = names.type_ref(&ml.member.tref, anon_lifetime());
        let offset = ml.offset as u32;
        let fieldname = names.struct_member(&ml.member.name);
        quote! {
            #type_::validate(
                &ptr.cast(#offset).map_err(|e|
                    ::memory::GuestError::InField{
                        typename: stringify!(#ident).to_owned(),
                        field: stringify!(#fieldname).to_owned(),
                        err: Box::new(e),
                    })?
                ).map_err(|e|
                    ::memory::GuestError::InField {
                        typename: stringify!(#ident).to_owned(),
                        field: stringify!(#fieldname).to_owned(),
                        err: Box::new(e),
                    })?;
        }
    });

    quote! {
        #[repr(C)]
        #[derive(Copy, Clone, Debug, ::std::hash::Hash, Eq, PartialEq)]
        pub struct #ident {
            #(#member_decls),*
        }

        impl ::memory::GuestType for #ident {
            fn size() -> u32 {
                #size
            }
            fn align() -> u32 {
                #align
            }
            fn name() -> String {
                stringify!(#ident).to_owned()
            }
            fn validate(ptr: &::memory::GuestPtr<#ident>) -> Result<(), ::memory::GuestError> {
                #(#member_valids)*
                Ok(())
            }
        }
        impl ::memory::GuestTypeCopy for #ident {}
    }
}

fn define_ptr_struct(names: &Names, name: &witx::Id, s: &witx::StructDatatype) -> TokenStream {
    let ident = names.type_(name);
    let size = s.mem_size_align().size as u32;
    let align = s.mem_size_align().align as u32;

    let member_names = s.members.iter().map(|m| names.struct_member(&m.name));
    let member_decls = s.members.iter().map(|m| {
        let name = names.struct_member(&m.name);
        let type_ = match &m.tref {
            witx::TypeRef::Name(nt) => names.type_(&nt.name),
            witx::TypeRef::Value(ty) => match &**ty {
                witx::Type::Builtin(builtin) => names.builtin_type(*builtin),
                witx::Type::Pointer(pointee) => {
                    let pointee_type = names.type_ref(&pointee, quote!('a));
                    quote!(::memory::GuestPtrMut<'a, #pointee_type>)
                }
                witx::Type::ConstPointer(pointee) => {
                    let pointee_type = names.type_ref(&pointee, quote!('a));
                    quote!(::memory::GuestPtr<'a, #pointee_type>)
                }
                _ => unimplemented!("other anonymous struct members"),
            },
        };
        quote!(pub #name: #type_)
    });
    let member_valids = s.member_layout().into_iter().map(|ml| {
        let type_ = match &ml.member.tref {
            witx::TypeRef::Name(nt) => names.type_(&nt.name),
            witx::TypeRef::Value(ty) => match &**ty {
                witx::Type::Builtin(builtin) => names.builtin_type(*builtin),
                witx::Type::Pointer(pointee) => {
                    let pointee_type = names.type_ref(&pointee, anon_lifetime());
                    quote!(::memory::GuestPtrMut::<#pointee_type>)
                }
                witx::Type::ConstPointer(pointee) => {
                    let pointee_type = names.type_ref(&pointee, anon_lifetime());
                    quote!(::memory::GuestPtr::<#pointee_type>)
                }
                _ => unimplemented!("other anonymous struct members"),
            },
        };
        let offset = ml.offset as u32;
        let fieldname = names.struct_member(&ml.member.name);
        quote! {
            #type_::validate(
                &ptr.cast(#offset).map_err(|e|
                    ::memory::GuestError::InField{
                        typename: stringify!(#ident).to_owned(),
                        field: stringify!(#fieldname).to_owned(),
                        err: Box::new(e),
                    })?
                ).map_err(|e|
                    ::memory::GuestError::InField {
                        typename: stringify!(#ident).to_owned(),
                        field: stringify!(#fieldname).to_owned(),
                        err: Box::new(e),
                    })?;
        }
    });

    let member_reads = s.member_layout().into_iter().map(|ml| {
        let name = names.struct_member(&ml.member.name);
        let offset = ml.offset as u32;
        match &ml.member.tref {
            witx::TypeRef::Name(nt) => {
                let type_ = names.type_(&nt.name);
                quote! {
                    let #name = #type_::read_from_guest(&location.cast(#offset)?)?;
                }
            }
            witx::TypeRef::Value(ty) => match &**ty {
                witx::Type::Builtin(builtin) => {
                    let type_ = names.builtin_type(*builtin);
                    quote! {
                        let #name = #type_::read_from_guest(&location.cast(#offset)?)?;
                    }
                }
                witx::Type::Pointer(pointee) => {
                    let pointee_type = names.type_ref(&pointee, anon_lifetime());
                    quote! {
                        let #name = ::memory::GuestPtrMut::<#pointee_type>::read_from_guest(&location.cast(#offset)?)?;
                    }
                }
                witx::Type::ConstPointer(pointee) => {
                    let pointee_type = names.type_ref(&pointee, anon_lifetime());
                    quote! {
                        let #name = ::memory::GuestPtr::<#pointee_type>::read_from_guest(&location.cast(#offset)?)?;
                    }
                }
                _ => unimplemented!("other anonymous struct members"),
            },
        }
    });

    let member_writes = s.member_layout().into_iter().map(|ml| {
        let name = names.struct_member(&ml.member.name);
        let offset = ml.offset as u32;
        quote!( self.#name.write_to_guest(&location.cast(#offset).expect("cast to inner member")); )
    });

    quote! {
        #[derive(Clone)]
        pub struct #ident<'a> {
            #(#member_decls),*
        }

        impl<'a> ::memory::GuestType for #ident<'a> {
            fn size() -> u32 {
                #size
            }
            fn align() -> u32 {
                #align
            }
            fn name() -> String {
                stringify!(#ident).to_owned()
            }
            fn validate(ptr: &::memory::GuestPtr<#ident>) -> Result<(), ::memory::GuestError> {
                #(#member_valids)*
                Ok(())
            }
        }
        impl<'a> ::memory::GuestTypePtr<'a> for #ident<'a> {
            fn read_from_guest(location: &::memory::GuestPtr<'a, #ident<'a>>) -> Result<#ident<'a>, ::memory::GuestError> {
                #(#member_reads)*
                Ok(#ident { #(#member_names),* })
            }
            fn write_to_guest(&self, location: &::memory::GuestPtrMut<'a, Self>) {
                #(#member_writes)*
            }
        }
    }
}

fn define_witx_pointer(
    names: &Names,
    name: &witx::Id,
    pointer_type: TokenStream,
    pointee: &witx::TypeRef,
) -> TokenStream {
    let ident = names.type_(name);
    let pointee_type = names.type_ref(pointee, quote!('a));

    quote!(pub type #ident<'a> = #pointer_type<'a, #pointee_type>;)
}

fn int_repr_tokens(int_repr: witx::IntRepr) -> TokenStream {
    match int_repr {
        witx::IntRepr::U8 => quote!(u8),
        witx::IntRepr::U16 => quote!(u16),
        witx::IntRepr::U32 => quote!(u32),
        witx::IntRepr::U64 => quote!(u64),
    }
}
fn atom_token(atom: witx::AtomType) -> TokenStream {
    match atom {
        witx::AtomType::I32 => quote!(i32),
        witx::AtomType::I64 => quote!(i64),
        witx::AtomType::F32 => quote!(f32),
        witx::AtomType::F64 => quote!(f64),
    }
}

pub fn anon_lifetime() -> TokenStream {
    quote!('_)
}
