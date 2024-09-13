use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::TokenStream;
use quote::quote;
use std::collections::HashMap;
use syn::{parse2, parse_str, Arm, Ident, ItemImpl, Type};

use crate::models::{
  OpenXmlSchema, OpenXmlSchemaType, OpenXmlSchemaTypeAttribute, OpenXmlSchemaTypeChild,
};
use crate::utils::{escape_snake_case, escape_upper_camel_case};
use crate::GenContext;

pub fn gen_serializer(schema: &OpenXmlSchema, context: &GenContext) -> TokenStream {
  let mut token_stream_list: Vec<ItemImpl> = vec![];

  let schema_namespace = context
    .uri_namespace_map
    .get(schema.target_namespace.as_str())
    .ok_or(format!("{:?}", schema.target_namespace))
    .unwrap();

  let scheme_mod = context
    .prefix_schema_mod_map
    .get(schema_namespace.prefix.as_str())
    .ok_or(format!("{:?}", schema_namespace.prefix))
    .unwrap();

  for e in &schema.enums {
    let enum_type: Type = parse_str(&format!(
      "crate::schemas::{}::{}",
      scheme_mod,
      e.name.to_upper_camel_case()
    ))
    .unwrap();

    let mut variants: Vec<Arm> = vec![];

    for facet in &e.facets {
      let variant_rename = &facet.value;

      let variant_ident: Ident = if facet.name.is_empty() {
        parse_str(&escape_upper_camel_case(facet.value.to_upper_camel_case())).unwrap()
      } else {
        parse_str(&escape_upper_camel_case(facet.name.to_upper_camel_case())).unwrap()
      };

      variants.push(
        parse2(quote! {
           Self::#variant_ident => #variant_rename.to_string(),
        })
        .unwrap(),
      )
    }

    token_stream_list.push(
      parse2(quote! {
        impl #enum_type {
          #[allow(clippy::inherent_to_string)]
          pub fn to_string(&self) -> String {
            match self {
              #( #variants )*
            }
          }
        }
      })
      .unwrap(),
    )
  }

  for t in &schema.types {
    if t.is_abstract {
      continue;
    }

    let struct_type: Type = parse_str(&format!(
      "crate::schemas::{}::{}",
      scheme_mod,
      t.class_name.to_upper_camel_case()
    ))
    .unwrap();

    let child_choice_enum_type: Type = parse_str(&format!(
      "crate::schemas::{}::{}ChildChoice",
      scheme_mod,
      t.class_name.to_upper_camel_case()
    ))
    .unwrap();

    let name_list: Vec<&str> = t.name.split('/').collect();

    let rename_ser_str = name_list.last().ok_or(format!("{:?}", t.name)).unwrap();

    let rename_list: Vec<&str> = rename_ser_str.split(':').collect();

    let rename_de_str = rename_list.last().ok_or(format!("{:?}", t.name)).unwrap();

    let end_tag_writer;

    let end_writer;

    let mut variants: Vec<TokenStream> = vec![];

    let mut children_writer = quote! {};

    let mut child_arms: Vec<Arm> = vec![];

    for attr in &t.attributes {
      variants.push(gen_attr(attr));
    }

    if t.base_class == "OpenXmlLeafTextElement" {
      children_writer = quote! {
        if let Some(child) = &self.child {
          writer.write_str(&quick_xml::escape::escape(&child.to_string()))?;
        }
      };

      end_tag_writer = quote! {
        writer.write_char('>')?;
      };

      end_writer = quote! {
        writer.write_str("</")?;

        if with_xmlns {
          writer.write_str(#rename_ser_str)?;
        } else {
          writer.write_str(#rename_de_str)?;
        }

        writer.write_char('>')?;
      };
    } else if t.base_class == "OpenXmlLeafElement" {
      children_writer = quote! {};

      end_tag_writer = quote! {};

      end_writer = quote! {
        writer.write_str("/>")?;
      };
    } else if t.base_class == "OpenXmlCompositeElement"
      || t.base_class == "CustomXmlElement"
      || t.base_class == "OpenXmlPartRootElement"
      || t.base_class == "SdtElement"
    {
      for child in &t.children {
        child_arms.push(gen_child_arm(child, &child_choice_enum_type));
      }

      if child_arms.is_empty() {
        end_tag_writer = quote! {};

        end_writer = quote! {
          writer.write_str("/>")?;
        };
      } else {
        children_writer = quote! {
          for child in &self.children {
            let child_str = match child {
              #( #child_arms )*
            };

            writer.write_str(&child_str)?;
          }
        };

        end_tag_writer = quote! {
          writer.write_char('>')?;
        };

        end_writer = quote! {
          writer.write_str("</")?;

          if with_xmlns {
            writer.write_str(#rename_ser_str)?;
          } else {
            writer.write_str(#rename_de_str)?;
          }

          writer.write_char('>')?;
        };
      }
    } else if t.is_derived {
      let base_class_type = context
        .type_base_class_type_map
        .get(t.base_class.as_str())
        .ok_or(format!("{:?}", t.base_class))
        .unwrap();

      for attr in &base_class_type.attributes {
        variants.push(gen_attr(attr));
      }

      let mut children_map: HashMap<&str, OpenXmlSchemaTypeChild> = HashMap::new();

      for c in &t.children {
        children_map.insert(&c.name, c.clone());
      }

      for c in &base_class_type.children {
        children_map.insert(&c.name, c.clone());
      }

      let children: Vec<OpenXmlSchemaTypeChild> = children_map.into_values().collect();

      for child in &children {
        child_arms.push(gen_child_arm(child, &child_choice_enum_type));
      }

      if children.is_empty() {
        if base_class_type.base_class == "OpenXmlLeafTextElement" {
          children_writer = quote! {
            if let Some(child) = &self.child {
              writer.write_str(&quick_xml::escape::escape(&child.to_string()))?;
            }
          };

          end_tag_writer = quote! {
            writer.write_char('>')?;
          };

          end_writer = quote! {
            writer.write_str("</")?;

            if with_xmlns {
              writer.write_str(#rename_ser_str)?;
            } else {
              writer.write_str(#rename_de_str)?;
            }

            writer.write_char('>')?;
          };
        } else {
          end_tag_writer = quote! {};

          end_writer = quote! {
            writer.write_str("/>")?;
          };
        }
      } else {
        children_writer = quote! {
          for child in &self.children {
            let child_str = match child {
              #( #child_arms )*
            };

            writer.write_str(&child_str)?;
          }
        };

        end_tag_writer = quote! {
          writer.write_char('>')?;
        };

        end_writer = quote! {
          writer.write_str("</")?;

          if with_xmlns {
            writer.write_str(#rename_ser_str)?;
          } else {
            writer.write_str(#rename_de_str)?;
          }

          writer.write_char('>')?;
        };
      }
    } else {
      panic!("{:?}", t);
    };

    let attr_writer = quote! {
      #( #variants )*
    };

    let xmlns_attr_writer = gen_xmlns(t);

    token_stream_list.push(
      parse2(quote! {
        impl #struct_type {
          #[allow(clippy::inherent_to_string)]
          pub fn to_string(&self, with_xmlns: bool) -> Result<String, crate::common::SdkError> {
            use std::fmt::Write;

            let mut writer = String::new();

            writer.write_char('<')?;

            if with_xmlns {
              writer.write_str(#rename_ser_str)?;
            } else {
              writer.write_str(#rename_de_str)?;
            }

            #xmlns_attr_writer

            #attr_writer

            #end_tag_writer

            #children_writer

            #end_writer

            Ok(writer)
          }
        }
      })
      .unwrap(),
    )
  }

  quote! {
    #( #token_stream_list )*
  }
}

fn gen_attr(attr: &OpenXmlSchemaTypeAttribute) -> TokenStream {
  let attr_rename_ser_str = if attr.q_name.starts_with(':') {
    &attr.q_name[1..attr.q_name.len()]
  } else {
    &attr.q_name
  };

  let attr_name_ident: Ident = if attr.property_name.is_empty() {
    parse_str(&escape_snake_case(attr.q_name.to_snake_case())).unwrap()
  } else {
    parse_str(&escape_snake_case(attr.property_name.to_snake_case())).unwrap()
  };

  let mut required = false;

  for validator in &attr.validators {
    if validator.name == "RequiredValidator" {
      required = true;
    }
  }

  if required {
    quote! {
      writer.write_char(' ')?;
      writer.write_str(#attr_rename_ser_str)?;
      writer.write_str("=\"")?;
      writer.write_str(&quick_xml::escape::escape(&self.#attr_name_ident.to_string()))?;
      writer.write_char('"')?;
    }
  } else {
    quote! {
      if let Some(#attr_name_ident) = &self.#attr_name_ident {
        writer.write_char(' ')?;
        writer.write_str(#attr_rename_ser_str)?;
        writer.write_str("=\"")?;
        writer.write_str(&quick_xml::escape::escape(&#attr_name_ident.to_string()))?;
        writer.write_char('"')?;
      }
    }
  }
}

fn gen_xmlns(t: &OpenXmlSchemaType) -> TokenStream {
  match t.part.as_str() {
    "WorkbookPart" => quote! {
      writer.write_str(r#" mc:Ignorable="x15" xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:x15="http://schemas.microsoft.com/office/spreadsheetml/2010/11/main" xmlns:xr="http://schemas.microsoft.com/office/spreadsheetml/2014/revision""#)?;
    },
    _ => quote! {},
  }
}

fn gen_child_arm(child: &OpenXmlSchemaTypeChild, child_choice_enum_type: &Type) -> Arm {
  let child_name_list: Vec<&str> = child.name.split('/').collect();

  let child_rename_ser_str = child_name_list
    .last()
    .ok_or(format!("{:?}", child.name))
    .unwrap();

  let child_variant_name_ident: Ident =
    parse_str(&child_rename_ser_str.to_upper_camel_case()).unwrap();

  parse2(quote! {
    #child_choice_enum_type::#child_variant_name_ident(child) => child.to_string(with_xmlns)?,
  })
  .unwrap()
}