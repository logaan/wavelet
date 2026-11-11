#[allow(dead_code, clippy::all)]
pub mod wavelet {
  pub mod meta {

    #[allow(dead_code, async_fn_in_trait, unused_imports, clippy::all)]
    pub mod code {
      #[used]
      #[doc(hidden)]
      static __FORCE_SECTION_REF: fn() =
      super::super::super::__link_custom_section_describing_imports;

      use super::super::super::_rt;
      pub type NodeId = u32;
      #[derive(Clone)]
      pub enum Node {
        BoolVal(bool),
        IntVal(i64),
        DecVal(f64),
        CharVal(char),
        StrVal(_rt::String),
        Sym(_rt::String),
        Tup(_rt::Vec::<NodeId>),

        Lst(_rt::Vec::<NodeId>),
        Rec(_rt::Vec::<(_rt::String,NodeId,)>),
        Flg(_rt::Vec::<_rt::String>),
      }
      impl ::core::fmt::Debug for Node {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
          match self {
            Node::BoolVal(e) => {
              f.debug_tuple("Node::BoolVal").field(e).finish()
            }
            Node::IntVal(e) => {
              f.debug_tuple("Node::IntVal").field(e).finish()
            }
            Node::DecVal(e) => {
              f.debug_tuple("Node::DecVal").field(e).finish()
            }
            Node::CharVal(e) => {
              f.debug_tuple("Node::CharVal").field(e).finish()
            }
            Node::StrVal(e) => {
              f.debug_tuple("Node::StrVal").field(e).finish()
            }
            Node::Sym(e) => {
              f.debug_tuple("Node::Sym").field(e).finish()
            }
            Node::Tup(e) => {
              f.debug_tuple("Node::Tup").field(e).finish()
            }
            Node::Lst(e) => {
              f.debug_tuple("Node::Lst").field(e).finish()
            }
            Node::Rec(e) => {
              f.debug_tuple("Node::Rec").field(e).finish()
            }
            Node::Flg(e) => {
              f.debug_tuple("Node::Flg").field(e).finish()
            }
          }
        }
      }
      #[derive(Clone)]
      pub struct Tree {
        pub nodes: _rt::Vec::<Node>,
        pub root: NodeId,
        pub spans: _rt::Vec::<(u32,u32,)>,
      }
      impl ::core::fmt::Debug for Tree {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
          f.debug_struct("Tree").field("nodes", &self.nodes).field("root", &self.root).field("spans", &self.spans).finish()
        }
      }

    }

  }
}
#[allow(dead_code, clippy::all)]
pub mod exports {
  pub mod wavelet {
    pub mod meta {

      #[allow(dead_code, async_fn_in_trait, unused_imports, clippy::all)]
      pub mod macros {
        #[used]
        #[doc(hidden)]
        static __FORCE_SECTION_REF: fn() =
        super::super::super::super::__link_custom_section_describing_imports;

        use super::super::super::super::_rt;
        pub type Tree = super::super::super::super::wavelet::meta::code::Tree;
        #[doc(hidden)]
        #[allow(non_snake_case, unused_unsafe)]
        pub unsafe fn _export_manifest_cabi<T: Guest>() -> *mut u8 { unsafe {#[cfg(target_arch="wasm32")]
        _rt::run_ctors_once();let result0 = {
          T::manifest()
        };
        let ptr1 = (&raw mut _RET_AREA.0).cast::<u8>();
        let vec4 = result0;
        let len4 = vec4.len();
        let layout4 = _rt::alloc::Layout::from_size_align(vec4.len() * (3*::core::mem::size_of::<*const u8>()), ::core::mem::size_of::<*const u8>()).unwrap();
        let (result4, _cleanup4) = wit_bindgen::rt::Cleanup::new(layout4);if let Some(cleanup) = _cleanup4 { cleanup.forget(); }
        for (i, e) in vec4.into_iter().enumerate() {
          let base = result4.add(i * (3*::core::mem::size_of::<*const u8>()));
          {
            let (t2_0, t2_1, ) = e;
            let vec3 = (t2_0.into_bytes()).into_boxed_slice();
            let ptr3 = vec3.as_ptr().cast::<u8>();
            let len3 = vec3.len();
            ::core::mem::forget(vec3);
            *base.add(::core::mem::size_of::<*const u8>()).cast::<usize>() = len3;
            *base.add(0).cast::<*mut u8>() = ptr3.cast_mut();
            *base.add(2*::core::mem::size_of::<*const u8>()).cast::<i32>() = _rt::as_i32(t2_1);
          }
        }
        *ptr1.add(::core::mem::size_of::<*const u8>()).cast::<usize>() = len4;
        *ptr1.add(0).cast::<*mut u8>() = result4;
        ptr1
      } }
      #[doc(hidden)]
      #[allow(non_snake_case)]
      pub unsafe fn __post_return_manifest<T: Guest>(arg0: *mut u8,) { unsafe {
        let l0 = *arg0.add(0).cast::<*mut u8>();
        let l1 = *arg0.add(::core::mem::size_of::<*const u8>()).cast::<usize>();
        let base4 = l0;
        let len4 = l1;
        for i in 0..len4 {
          let base = base4.add(i * (3*::core::mem::size_of::<*const u8>()));
          {
            let l2 = *base.add(0).cast::<*mut u8>();
            let l3 = *base.add(::core::mem::size_of::<*const u8>()).cast::<usize>();
            _rt::cabi_dealloc(l2, l3, 1);
          }
        }
        _rt::cabi_dealloc(base4, len4 * (3*::core::mem::size_of::<*const u8>()), ::core::mem::size_of::<*const u8>());
      } }
      #[doc(hidden)]
      #[allow(non_snake_case, unused_unsafe)]
      pub unsafe fn _export_expand_cabi<T: Guest>(arg0: *mut u8,arg1: usize,arg2: *mut u8,arg3: usize,arg4: i32,arg5: *mut u8,arg6: usize,) -> *mut u8 { unsafe {#[cfg(target_arch="wasm32")]
      _rt::run_ctors_once();let result36 = {
        let len0 = arg1;
        let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
        let base32 = arg2;
        let len32 = arg3;
        let mut result32 = _rt::Vec::with_capacity(len32);
        for i in 0..len32 {
          let base = base32.add(i * (8+2*::core::mem::size_of::<*const u8>()));
          let e32 = {
            let l1 = i32::from(*base.add(0).cast::<u8>());
            use super::super::super::super::wavelet::meta::code::Node as V31;
            let v31 = match l1 {
              0 => {
                let e31 = {
                  let l2 = i32::from(*base.add(8).cast::<u8>());

                  _rt::bool_lift(l2 as u8)
                };
                V31::BoolVal(e31)
              }
              1 => {
                let e31 = {
                  let l3 = *base.add(8).cast::<i64>();

                  l3
                };
                V31::IntVal(e31)
              }
              2 => {
                let e31 = {
                  let l4 = *base.add(8).cast::<f64>();

                  l4
                };
                V31::DecVal(e31)
              }
              3 => {
                let e31 = {
                  let l5 = *base.add(8).cast::<i32>();

                  _rt::char_lift(l5 as u32)
                };
                V31::CharVal(e31)
              }
              4 => {
                let e31 = {
                  let l6 = *base.add(8).cast::<*mut u8>();
                  let l7 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  let len8 = l7;
                  let bytes8 = _rt::Vec::from_raw_parts(l6.cast(), len8, len8);

                  _rt::string_lift(bytes8)
                };
                V31::StrVal(e31)
              }
              5 => {
                let e31 = {
                  let l9 = *base.add(8).cast::<*mut u8>();
                  let l10 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  let len11 = l10;
                  let bytes11 = _rt::Vec::from_raw_parts(l9.cast(), len11, len11);

                  _rt::string_lift(bytes11)
                };
                V31::Sym(e31)
              }
              6 => {
                let e31 = {
                  let l12 = *base.add(8).cast::<*mut u8>();
                  let l13 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  let len14 = l13;

                  _rt::Vec::from_raw_parts(l12.cast(), len14, len14)
                };
                V31::Tup(e31)
              }
              7 => {
                let e31 = {
                  let l15 = *base.add(8).cast::<*mut u8>();
                  let l16 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  let len17 = l16;

                  _rt::Vec::from_raw_parts(l15.cast(), len17, len17)
                };
                V31::Lst(e31)
              }
              8 => {
                let e31 = {
                  let l18 = *base.add(8).cast::<*mut u8>();
                  let l19 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  let base24 = l18;
                  let len24 = l19;
                  let mut result24 = _rt::Vec::with_capacity(len24);
                  for i in 0..len24 {
                    let base = base24.add(i * (3*::core::mem::size_of::<*const u8>()));
                    let e24 = {
                      let l20 = *base.add(0).cast::<*mut u8>();
                      let l21 = *base.add(::core::mem::size_of::<*const u8>()).cast::<usize>();
                      let len22 = l21;
                      let bytes22 = _rt::Vec::from_raw_parts(l20.cast(), len22, len22);
                      let l23 = *base.add(2*::core::mem::size_of::<*const u8>()).cast::<i32>();

                      (_rt::string_lift(bytes22), l23 as u32)
                    };
                    result24.push(e24);
                  }
                  _rt::cabi_dealloc(base24, len24 * (3*::core::mem::size_of::<*const u8>()), ::core::mem::size_of::<*const u8>());

                  result24
                };
                V31::Rec(e31)
              }
              n => {
                debug_assert_eq!(n, 9, "invalid enum discriminant");
                let e31 = {
                  let l25 = *base.add(8).cast::<*mut u8>();
                  let l26 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  let base30 = l25;
                  let len30 = l26;
                  let mut result30 = _rt::Vec::with_capacity(len30);
                  for i in 0..len30 {
                    let base = base30.add(i * (2*::core::mem::size_of::<*const u8>()));
                    let e30 = {
                      let l27 = *base.add(0).cast::<*mut u8>();
                      let l28 = *base.add(::core::mem::size_of::<*const u8>()).cast::<usize>();
                      let len29 = l28;
                      let bytes29 = _rt::Vec::from_raw_parts(l27.cast(), len29, len29);

                      _rt::string_lift(bytes29)
                    };
                    result30.push(e30);
                  }
                  _rt::cabi_dealloc(base30, len30 * (2*::core::mem::size_of::<*const u8>()), ::core::mem::size_of::<*const u8>());

                  result30
                };
                V31::Flg(e31)
              }
            };

            v31
          };
          result32.push(e32);
        }
        _rt::cabi_dealloc(base32, len32 * (8+2*::core::mem::size_of::<*const u8>()), 8);
        let base35 = arg5;
        let len35 = arg6;
        let mut result35 = _rt::Vec::with_capacity(len35);
        for i in 0..len35 {
          let base = base35.add(i * 8);
          let e35 = {
            let l33 = *base.add(0).cast::<i32>();
            let l34 = *base.add(4).cast::<i32>();

            (l33 as u32, l34 as u32)
          };
          result35.push(e35);
        }
        _rt::cabi_dealloc(base35, len35 * 8, 4);
        T::expand(_rt::string_lift(bytes0), super::super::super::super::wavelet::meta::code::Tree{
          nodes: result32,
          root: arg4 as u32,
          spans: result35,
        })
      };
      let ptr37 = (&raw mut _RET_AREA.0).cast::<u8>();
      match result36 {
        Ok(e) => { {
          *ptr37.add(0).cast::<u8>() = (0i32) as u8;
          let super::super::super::super::wavelet::meta::code::Tree{ nodes:nodes38, root:root38, spans:spans38, } = e;
          let vec49 = nodes38;
          let len49 = vec49.len();
          let layout49 = _rt::alloc::Layout::from_size_align(vec49.len() * (8+2*::core::mem::size_of::<*const u8>()), 8).unwrap();
          let (result49, _cleanup49) = wit_bindgen::rt::Cleanup::new(layout49);if let Some(cleanup) = _cleanup49 { cleanup.forget(); }
          for (i, e) in vec49.into_iter().enumerate() {
            let base = result49.add(i * (8+2*::core::mem::size_of::<*const u8>()));
            {
              use super::super::super::super::wavelet::meta::code::Node as V48;
              match e {
                V48::BoolVal(e) => {
                  *base.add(0).cast::<u8>() = (0i32) as u8;
                  *base.add(8).cast::<u8>() = (match e { true => 1, false => 0 }) as u8;
                },
                V48::IntVal(e) => {
                  *base.add(0).cast::<u8>() = (1i32) as u8;
                  *base.add(8).cast::<i64>() = _rt::as_i64(e);
                },
                V48::DecVal(e) => {
                  *base.add(0).cast::<u8>() = (2i32) as u8;
                  *base.add(8).cast::<f64>() = _rt::as_f64(e);
                },
                V48::CharVal(e) => {
                  *base.add(0).cast::<u8>() = (3i32) as u8;
                  *base.add(8).cast::<i32>() = _rt::as_i32(e);
                },
                V48::StrVal(e) => {
                  *base.add(0).cast::<u8>() = (4i32) as u8;
                  let vec39 = (e.into_bytes()).into_boxed_slice();
                  let ptr39 = vec39.as_ptr().cast::<u8>();
                  let len39 = vec39.len();
                  ::core::mem::forget(vec39);
                  *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>() = len39;
                  *base.add(8).cast::<*mut u8>() = ptr39.cast_mut();
                },
                V48::Sym(e) => {
                  *base.add(0).cast::<u8>() = (5i32) as u8;
                  let vec40 = (e.into_bytes()).into_boxed_slice();
                  let ptr40 = vec40.as_ptr().cast::<u8>();
                  let len40 = vec40.len();
                  ::core::mem::forget(vec40);
                  *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>() = len40;
                  *base.add(8).cast::<*mut u8>() = ptr40.cast_mut();
                },
                V48::Tup(e) => {
                  *base.add(0).cast::<u8>() = (6i32) as u8;
                  let vec41 = (e).into_boxed_slice();
                  let ptr41 = vec41.as_ptr().cast::<u8>();
                  let len41 = vec41.len();
                  ::core::mem::forget(vec41);
                  *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>() = len41;
                  *base.add(8).cast::<*mut u8>() = ptr41.cast_mut();
                },
                V48::Lst(e) => {
                  *base.add(0).cast::<u8>() = (7i32) as u8;
                  let vec42 = (e).into_boxed_slice();
                  let ptr42 = vec42.as_ptr().cast::<u8>();
                  let len42 = vec42.len();
                  ::core::mem::forget(vec42);
                  *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>() = len42;
                  *base.add(8).cast::<*mut u8>() = ptr42.cast_mut();
                },
                V48::Rec(e) => {
                  *base.add(0).cast::<u8>() = (8i32) as u8;
                  let vec45 = e;
                  let len45 = vec45.len();
                  let layout45 = _rt::alloc::Layout::from_size_align(vec45.len() * (3*::core::mem::size_of::<*const u8>()), ::core::mem::size_of::<*const u8>()).unwrap();
                  let (result45, _cleanup45) = wit_bindgen::rt::Cleanup::new(layout45);if let Some(cleanup) = _cleanup45 { cleanup.forget(); }
                  for (i, e) in vec45.into_iter().enumerate() {
                    let base = result45.add(i * (3*::core::mem::size_of::<*const u8>()));
                    {
                      let (t43_0, t43_1, ) = e;
                      let vec44 = (t43_0.into_bytes()).into_boxed_slice();
                      let ptr44 = vec44.as_ptr().cast::<u8>();
                      let len44 = vec44.len();
                      ::core::mem::forget(vec44);
                      *base.add(::core::mem::size_of::<*const u8>()).cast::<usize>() = len44;
                      *base.add(0).cast::<*mut u8>() = ptr44.cast_mut();
                      *base.add(2*::core::mem::size_of::<*const u8>()).cast::<i32>() = _rt::as_i32(t43_1);
                    }
                  }
                  *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>() = len45;
                  *base.add(8).cast::<*mut u8>() = result45;
                },
                V48::Flg(e) => {
                  *base.add(0).cast::<u8>() = (9i32) as u8;
                  let vec47 = e;
                  let len47 = vec47.len();
                  let layout47 = _rt::alloc::Layout::from_size_align(vec47.len() * (2*::core::mem::size_of::<*const u8>()), ::core::mem::size_of::<*const u8>()).unwrap();
                  let (result47, _cleanup47) = wit_bindgen::rt::Cleanup::new(layout47);if let Some(cleanup) = _cleanup47 { cleanup.forget(); }
                  for (i, e) in vec47.into_iter().enumerate() {
                    let base = result47.add(i * (2*::core::mem::size_of::<*const u8>()));
                    {
                      let vec46 = (e.into_bytes()).into_boxed_slice();
                      let ptr46 = vec46.as_ptr().cast::<u8>();
                      let len46 = vec46.len();
                      ::core::mem::forget(vec46);
                      *base.add(::core::mem::size_of::<*const u8>()).cast::<usize>() = len46;
                      *base.add(0).cast::<*mut u8>() = ptr46.cast_mut();
                    }
                  }
                  *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>() = len47;
                  *base.add(8).cast::<*mut u8>() = result47;
                },
              }
            }
          }
          *ptr37.add(2*::core::mem::size_of::<*const u8>()).cast::<usize>() = len49;
          *ptr37.add(::core::mem::size_of::<*const u8>()).cast::<*mut u8>() = result49;
          *ptr37.add(3*::core::mem::size_of::<*const u8>()).cast::<i32>() = _rt::as_i32(root38);
          let vec51 = spans38;
          let len51 = vec51.len();
          let layout51 = _rt::alloc::Layout::from_size_align(vec51.len() * 8, 4).unwrap();
          let (result51, _cleanup51) = wit_bindgen::rt::Cleanup::new(layout51);if let Some(cleanup) = _cleanup51 { cleanup.forget(); }
          for (i, e) in vec51.into_iter().enumerate() {
            let base = result51.add(i * 8);
            {
              let (t50_0, t50_1, ) = e;
              *base.add(0).cast::<i32>() = _rt::as_i32(t50_0);
              *base.add(4).cast::<i32>() = _rt::as_i32(t50_1);
            }
          }
          *ptr37.add(5*::core::mem::size_of::<*const u8>()).cast::<usize>() = len51;
          *ptr37.add(4*::core::mem::size_of::<*const u8>()).cast::<*mut u8>() = result51;
        } },
        Err(e) => { {
          *ptr37.add(0).cast::<u8>() = (1i32) as u8;
          let vec52 = (e.into_bytes()).into_boxed_slice();
          let ptr52 = vec52.as_ptr().cast::<u8>();
          let len52 = vec52.len();
          ::core::mem::forget(vec52);
          *ptr37.add(2*::core::mem::size_of::<*const u8>()).cast::<usize>() = len52;
          *ptr37.add(::core::mem::size_of::<*const u8>()).cast::<*mut u8>() = ptr52.cast_mut();
        } },
      };ptr37
    } }
    #[doc(hidden)]
    #[allow(non_snake_case)]
    pub unsafe fn __post_return_expand<T: Guest>(arg0: *mut u8,) { unsafe {
      let l0 = i32::from(*arg0.add(0).cast::<u8>());
      match l0 {
        0 => {
          let l1 = *arg0.add(::core::mem::size_of::<*const u8>()).cast::<*mut u8>();
          let l2 = *arg0.add(2*::core::mem::size_of::<*const u8>()).cast::<usize>();
          let base24 = l1;
          let len24 = l2;
          for i in 0..len24 {
            let base = base24.add(i * (8+2*::core::mem::size_of::<*const u8>()));
            {
              let l3 = i32::from(*base.add(0).cast::<u8>());
              match l3 {
                0 => (),
                1 => (),
                2 => (),
                3 => (),
                4 => {
                  let l4 = *base.add(8).cast::<*mut u8>();
                  let l5 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  _rt::cabi_dealloc(l4, l5, 1);
                },
                5 => {
                  let l6 = *base.add(8).cast::<*mut u8>();
                  let l7 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  _rt::cabi_dealloc(l6, l7, 1);
                },
                6 => {
                  let l8 = *base.add(8).cast::<*mut u8>();
                  let l9 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  let base10 = l8;
                  let len10 = l9;
                  _rt::cabi_dealloc(base10, len10 * 4, 4);
                },
                7 => {
                  let l11 = *base.add(8).cast::<*mut u8>();
                  let l12 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  let base13 = l11;
                  let len13 = l12;
                  _rt::cabi_dealloc(base13, len13 * 4, 4);
                },
                8 => {
                  let l14 = *base.add(8).cast::<*mut u8>();
                  let l15 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  let base18 = l14;
                  let len18 = l15;
                  for i in 0..len18 {
                    let base = base18.add(i * (3*::core::mem::size_of::<*const u8>()));
                    {
                      let l16 = *base.add(0).cast::<*mut u8>();
                      let l17 = *base.add(::core::mem::size_of::<*const u8>()).cast::<usize>();
                      _rt::cabi_dealloc(l16, l17, 1);
                    }
                  }
                  _rt::cabi_dealloc(base18, len18 * (3*::core::mem::size_of::<*const u8>()), ::core::mem::size_of::<*const u8>());
                },
                _ => {
                  let l19 = *base.add(8).cast::<*mut u8>();
                  let l20 = *base.add(8+1*::core::mem::size_of::<*const u8>()).cast::<usize>();
                  let base23 = l19;
                  let len23 = l20;
                  for i in 0..len23 {
                    let base = base23.add(i * (2*::core::mem::size_of::<*const u8>()));
                    {
                      let l21 = *base.add(0).cast::<*mut u8>();
                      let l22 = *base.add(::core::mem::size_of::<*const u8>()).cast::<usize>();
                      _rt::cabi_dealloc(l21, l22, 1);
                    }
                  }
                  _rt::cabi_dealloc(base23, len23 * (2*::core::mem::size_of::<*const u8>()), ::core::mem::size_of::<*const u8>());
                },
              }
            }
          }
          _rt::cabi_dealloc(base24, len24 * (8+2*::core::mem::size_of::<*const u8>()), 8);
          let l25 = *arg0.add(4*::core::mem::size_of::<*const u8>()).cast::<*mut u8>();
          let l26 = *arg0.add(5*::core::mem::size_of::<*const u8>()).cast::<usize>();
          let base27 = l25;
          let len27 = l26;
          _rt::cabi_dealloc(base27, len27 * 8, 4);
        },
        _ => {
          let l28 = *arg0.add(::core::mem::size_of::<*const u8>()).cast::<*mut u8>();
          let l29 = *arg0.add(2*::core::mem::size_of::<*const u8>()).cast::<usize>();
          _rt::cabi_dealloc(l28, l29, 1);
        },
      }
    } }
    pub trait Guest {

      #[allow(async_fn_in_trait)]
      fn manifest() -> _rt::Vec::<(_rt::String,u32,)>;

      #[allow(async_fn_in_trait)]
      fn expand(name: _rt::String,args: Tree,) -> Result<Tree,_rt::String>;
    }
    #[doc(hidden)]

    macro_rules! __export_wavelet_meta_macros_0_1_0_cabi{
      ($ty:ident with_types_in $($path_to_types:tt)*) => (const _: () = {

        #[unsafe(export_name = "wavelet:meta/macros@0.1.0#manifest")]
        unsafe extern "C" fn export_manifest() -> *mut u8 {
          unsafe { $($path_to_types)*::_export_manifest_cabi::<$ty>() }
        }
        #[unsafe(export_name = "cabi_post_wavelet:meta/macros@0.1.0#manifest")]
        unsafe extern "C" fn _post_return_manifest(arg0: *mut u8,) {
          unsafe { $($path_to_types)*::__post_return_manifest::<$ty>(arg0) }
        }
        #[unsafe(export_name = "wavelet:meta/macros@0.1.0#expand")]
        unsafe extern "C" fn export_expand(arg0: *mut u8,arg1: usize,arg2: *mut u8,arg3: usize,arg4: i32,arg5: *mut u8,arg6: usize,) -> *mut u8 {
          unsafe { $($path_to_types)*::_export_expand_cabi::<$ty>(arg0, arg1, arg2, arg3, arg4, arg5, arg6) }
        }
        #[unsafe(export_name = "cabi_post_wavelet:meta/macros@0.1.0#expand")]
        unsafe extern "C" fn _post_return_expand(arg0: *mut u8,) {
          unsafe { $($path_to_types)*::__post_return_expand::<$ty>(arg0) }
        }
      };);
    }
    #[doc(hidden)]
    pub(crate) use __export_wavelet_meta_macros_0_1_0_cabi;

    #[cfg_attr(target_pointer_width="64", repr(align(8)))]
    #[cfg_attr(target_pointer_width="32", repr(align(4)))]
    struct _RetArea([::core::mem::MaybeUninit::<u8>; 6*::core::mem::size_of::<*const u8>()]);
    static mut _RET_AREA: _RetArea = _RetArea([::core::mem::MaybeUninit::uninit(); 6*::core::mem::size_of::<*const u8>()]);

  }

}
}
}
mod _rt {
  #![allow(dead_code, clippy::all)]
  pub use alloc_crate::string::String;
  pub use alloc_crate::vec::Vec;

  #[cfg(target_arch = "wasm32")]
  pub fn run_ctors_once() {
    wit_bindgen::rt::run_ctors_once();
  }

  pub fn as_i32<T: AsI32>(t: T) -> i32 {
    t.as_i32()
  }

  pub trait AsI32 {
    fn as_i32(self) -> i32;
  }

  impl<'a, T: Copy + AsI32> AsI32 for &'a T {
    fn as_i32(self) -> i32 {
      (*self).as_i32()
    }
  }

  impl AsI32 for i32 {
    #[inline]
    fn as_i32(self) -> i32 {
      self as i32
    }
  }

  impl AsI32 for u32 {
    #[inline]
    fn as_i32(self) -> i32 {
      self as i32
    }
  }

  impl AsI32 for i16 {
    #[inline]
    fn as_i32(self) -> i32 {
      self as i32
    }
  }

  impl AsI32 for u16 {
    #[inline]
    fn as_i32(self) -> i32 {
      self as i32
    }
  }

  impl AsI32 for i8 {
    #[inline]
    fn as_i32(self) -> i32 {
      self as i32
    }
  }

  impl AsI32 for u8 {
    #[inline]
    fn as_i32(self) -> i32 {
      self as i32
    }
  }

  impl AsI32 for char {
    #[inline]
    fn as_i32(self) -> i32 {
      self as i32
    }
  }

  impl AsI32 for usize {
    #[inline]
    fn as_i32(self) -> i32 {
      self as i32
    }
  }
  pub use alloc_crate::alloc;
  pub unsafe fn cabi_dealloc(ptr: *mut u8, size: usize, align: usize) {
    if size == 0 {
      return;
    }
    unsafe {
      let layout = alloc::Layout::from_size_align_unchecked(size, align);
      alloc::dealloc(ptr, layout);
    }
  }
  pub unsafe fn string_lift(bytes: Vec<u8>) -> String {
    if cfg!(debug_assertions) {
      String::from_utf8(bytes).unwrap()
    } else {
      unsafe { String::from_utf8_unchecked(bytes) }
    }
  }
  pub unsafe fn bool_lift(val: u8) -> bool {
    if cfg!(debug_assertions) {
      match val {
        0 => false,
        1 => true,
        _ => panic!("invalid bool discriminant"),
      }
    } else {
      val != 0
    }
  }
  pub unsafe fn char_lift(val: u32) -> char {
    if cfg!(debug_assertions) {
      core::char::from_u32(val).unwrap()
    } else {
      unsafe { core::char::from_u32_unchecked(val) }
    }
  }

  pub fn as_i64<T: AsI64>(t: T) -> i64 {
    t.as_i64()
  }

  pub trait AsI64 {
    fn as_i64(self) -> i64;
  }

  impl<'a, T: Copy + AsI64> AsI64 for &'a T {
    fn as_i64(self) -> i64 {
      (*self).as_i64()
    }
  }

  impl AsI64 for i64 {
    #[inline]
    fn as_i64(self) -> i64 {
      self as i64
    }
  }

  impl AsI64 for u64 {
    #[inline]
    fn as_i64(self) -> i64 {
      self as i64
    }
  }

  pub fn as_f64<T: AsF64>(t: T) -> f64 {
    t.as_f64()
  }

  pub trait AsF64 {
    fn as_f64(self) -> f64;
  }

  impl<'a, T: Copy + AsF64> AsF64 for &'a T {
    fn as_f64(self) -> f64 {
      (*self).as_f64()
    }
  }

  impl AsF64 for f64 {
    #[inline]
    fn as_f64(self) -> f64 {
      self as f64
    }
  }
  extern crate alloc as alloc_crate;
}

#[allow(unused_macros)]
#[doc(hidden)]

macro_rules! __export_macro_lib_impl {
  ($ty:ident) => (self::export!($ty with_types_in self););
  ($ty:ident with_types_in $($path_to_types_root:tt)*) => (
  $($path_to_types_root)*::exports::wavelet::meta::macros::__export_wavelet_meta_macros_0_1_0_cabi!($ty with_types_in $($path_to_types_root)*::exports::wavelet::meta::macros);
  )
}
#[doc(inline)]
pub(crate) use __export_macro_lib_impl as export;

#[cfg(target_arch = "wasm32")]
#[unsafe(link_section = "component-type:wit-bindgen:0.43.0:wavelet:macros-fixture@0.1.0:macro-lib:encoded world")]
#[doc(hidden)]
#[allow(clippy::octal_escapes)]
pub static __WIT_BINDGEN_COMPONENT_TYPE: [u8; 503] = *b"\
\0asm\x0d\0\x01\0\0\x19\x16wit-component-encoding\x04\0\x07\xf7\x02\x01A\x02\x01\
A\x05\x01B\x0d\x01y\x04\0\x07node-id\x03\0\0\x01p\x01\x01o\x02s\x01\x01p\x03\x01\
ps\x01q\x0a\x08bool-val\x01\x7f\0\x07int-val\x01x\0\x07dec-val\x01u\0\x08char-va\
l\x01t\0\x07str-val\x01s\0\x03sym\x01s\0\x03tup\x01\x02\0\x03lst\x01\x02\0\x03re\
c\x01\x04\0\x03flg\x01\x05\0\x04\0\x04node\x03\0\x06\x01p\x07\x01o\x02yy\x01p\x09\
\x01r\x03\x05nodes\x08\x04root\x01\x05spans\x0a\x04\0\x04tree\x03\0\x0b\x03\0\x17\
wavelet:meta/code@0.1.0\x05\0\x02\x03\0\0\x04tree\x01B\x09\x02\x03\x02\x01\x01\x04\
\0\x04tree\x03\0\0\x01o\x02sy\x01p\x02\x01@\0\0\x03\x04\0\x08manifest\x01\x04\x01\
j\x01\x01\x01s\x01@\x02\x04names\x04args\x01\0\x05\x04\0\x06expand\x01\x06\x04\0\
\x19wavelet:meta/macros@0.1.0\x05\x02\x04\0&wavelet:macros-fixture/macro-lib@0.1\
.0\x04\0\x0b\x0f\x01\0\x09macro-lib\x03\0\0\0G\x09producers\x01\x0cprocessed-by\x02\
\x0dwit-component\x070.235.0\x10wit-bindgen-rust\x060.43.0";

#[inline(never)]
#[doc(hidden)]
pub fn __link_custom_section_describing_imports() {
  wit_bindgen::rt::maybe_link_cabi_realloc();
}

