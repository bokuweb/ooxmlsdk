use heck::{ToSnakeCase, ToUpperCamelCase};
use proc_macro2::TokenStream;
use quote::quote;
use std::collections::HashSet;
use syn::{parse_str, Arm, Ident, ItemFn, LitByteStr, Stmt, Type};

use crate::models::{
  OpenXmlNamespace, OpenXmlSchema, OpenXmlSchemaType, OpenXmlSchemaTypeAttribute,
  OpenXmlSchemaTypeChild,
};
use crate::utils::{escape_snake_case, escape_upper_camel_case};
use crate::GenContext;

pub fn gen_deserializer(schema: &OpenXmlSchema, context: &GenContext) -> TokenStream {
  let mut token_stream_list: Vec<TokenStream> = vec![];

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

    let mut variants: Vec<TokenStream> = vec![];

    for facet in &e.facets {
      let variant_rename = &facet.value;

      let variant_ident: Ident = if facet.name.is_empty() {
        parse_str(&escape_upper_camel_case(facet.value.to_upper_camel_case())).unwrap()
      } else {
        parse_str(&escape_upper_camel_case(facet.name.to_upper_camel_case())).unwrap()
      };

      variants.push(quote! {
        #variant_rename => Ok(Self::#variant_ident),
      })
    }

    token_stream_list.push(quote! {
      impl #enum_type {
        pub fn from_str(s: &str) -> Result<Self, super::deserializers::DeError> {
          match s {
            #( #variants )*
            _ => Err(super::deserializers::DeError::UnknownError),
          }
        }
      }
    })
  }

  let from_str_fn = gen_from_str_fn();

  let from_reader_fn = gen_from_reader_fn();

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

    let deserialize_self_fn = if t.base_class == "OpenXmlLeafTextElement" {
      gen_open_xml_leaf_text_element_fn(t)
    } else if t.base_class == "OpenXmlLeafElement" {
      gen_open_xml_leaf_element_fn(t, schema_namespace, context)
    } else if t.base_class == "OpenXmlCompositeElement"
      || t.base_class == "CustomXmlElement"
      || t.base_class == "OpenXmlPartRootElement"
      || t.base_class == "SdtElement"
    {
      gen_open_xml_composite_element_fn(t, schema_namespace, context)
    } else if t.is_derived {
      gen_derived_fn(t)
    } else {
      panic!("{:?}", t);
    };

    let output = quote! {
      impl #struct_type {
        #from_str_fn

        #from_reader_fn

        #deserialize_self_fn
      }
    };

    // let syntax_tree = syn::parse2(output.clone()).unwrap();
    // let _ = prettyplease::unparse(&syntax_tree);

    token_stream_list.push(output);
  }

  quote! {
    #( #token_stream_list )*
  }
}

fn gen_from_str_fn() -> ItemFn {
  let token_stream = quote! {
    pub fn from_str(s: &str) -> Result<Self, super::deserializers::DeError> {
      let mut xml_reader = super::deserializers::SliceReader::new(quick_xml::Reader::from_str(s));

      Self::deserialize_self(&mut xml_reader, false)
    }
  };

  parse_str(&token_stream.to_string()).unwrap()
}

fn gen_from_reader_fn() -> ItemFn {
  let token_stream = quote! {
    pub fn from_reader<R: std::io::BufRead>(
      reader: R,
    ) -> Result<Self, super::deserializers::DeError> {
      let mut xml_reader =
        super::deserializers::IoReader::new(quick_xml::Reader::from_reader(reader));

      Self::deserialize_self(&mut xml_reader, false)
    }
  };

  parse_str(&token_stream.to_string()).unwrap()
}

fn gen_open_xml_leaf_element_fn(
  t: &OpenXmlSchemaType,
  schema_namespace: &OpenXmlNamespace,
  context: &GenContext,
) -> ItemFn {
  let name_list: Vec<&str> = t.name.split('/').collect();

  let rename_ser_str = name_list.last().ok_or(format!("{:?}", t.name)).unwrap();

  let rename_list: Vec<&str> = rename_ser_str.split(':').collect();

  let rename_de_str = rename_list.last().ok_or(format!("{:?}", t.name)).unwrap();

  let rename_ser_literal: LitByteStr = parse_str(&format!("b\"{}\"", rename_ser_str)).unwrap();

  let rename_de_literal: LitByteStr = parse_str(&format!("b\"{}\"", rename_de_str)).unwrap();

  let mut field_declaration_list: Vec<TokenStream> = vec![];
  let mut field_match_list: Vec<Arm> = vec![];
  let mut field_unwrap_list: Vec<TokenStream> = vec![];
  let mut field_init_list: Vec<TokenStream> = vec![];

  let xmlns_literal: LitByteStr =
    parse_str(&format!("b\"xmlns:{}\"", schema_namespace.prefix)).unwrap();

  field_match_list.push(
    parse_str(
      &quote! {
        #xmlns_literal => with_xmlns = true,
      }
      .to_string(),
    )
    .unwrap(),
  );

  for attr in &t.attributes {
    let attr_name_ident: Ident = if attr.property_name.is_empty() {
      parse_str(&escape_snake_case(attr.q_name.to_snake_case())).unwrap()
    } else {
      parse_str(&escape_snake_case(attr.property_name.to_snake_case())).unwrap()
    };

    field_declaration_list.push(quote! {
      let mut #attr_name_ident = None;
    });

    field_match_list.push(gen_field_match_arm(attr, context));

    let mut required = false;

    for validator in &attr.validators {
      if validator.name == "RequiredValidator" {
        required = true;
      }
    }

    if required {
      field_unwrap_list.push(quote! {
        let #attr_name_ident = #attr_name_ident.ok_or_else(|| super::deserializers::DeError::UnknownError)?;
      })
    }

    field_init_list.push(quote! {
      #attr_name_ident,
    })
  }

  let token_stream = quote! {
    pub fn deserialize_self<'de, R: super::deserializers::XmlReader<'de>>(
      xml_reader: &mut R,
      with_xmlns: bool,
    ) -> Result<Self, super::deserializers::DeError> {
      let mut with_xmlns = with_xmlns;

      if let quick_xml::events::Event::Empty(e) = xml_reader.next()? {
        #( #field_declaration_list )*

        for attr in e.attributes() {
          let attr = attr?;

          match attr.key.as_ref() {
            #( #field_match_list )*
            _ => (),
          }
        }

        if with_xmlns {
          if e.name().as_ref() != #rename_ser_literal {
            Err(super::deserializers::DeError::UnknownError)?;
          }
        } else if e.name().local_name().as_ref() != #rename_de_literal {
          Err(super::deserializers::DeError::UnknownError)?;
        }

        #( #field_unwrap_list )*

        Ok(Self {
          #( #field_init_list )*
        })
      } else {
        Err(super::deserializers::DeError::UnknownError)?
      }
    }
  };

  parse_str(&token_stream.to_string()).unwrap()
}

fn gen_open_xml_leaf_text_element_fn(_t: &OpenXmlSchemaType) -> ItemFn {
  let token_stream = quote! {
    pub fn deserialize_self<'de, R: super::deserializers::XmlReader<'de>>(
      xml_reader: &mut R,
      with_xmlns: bool,
    ) -> Result<Self, super::deserializers::DeError> {
      Err(super::deserializers::DeError::UnknownError)?
    }
  };

  parse_str(&token_stream.to_string()).unwrap()
}

fn gen_open_xml_composite_element_fn(
  t: &OpenXmlSchemaType,
  schema_namespace: &OpenXmlNamespace,
  context: &GenContext,
) -> ItemFn {
  let name_list: Vec<&str> = t.name.split('/').collect();

  let rename_ser_str = name_list.last().ok_or(format!("{:?}", t.name)).unwrap();

  let rename_list: Vec<&str> = rename_ser_str.split(':').collect();

  let rename_de_str = rename_list.last().ok_or(format!("{:?}", t.name)).unwrap();

  let rename_ser_literal: LitByteStr = parse_str(&format!("b\"{}\"", rename_ser_str)).unwrap();

  let rename_de_literal: LitByteStr = parse_str(&format!("b\"{}\"", rename_de_str)).unwrap();

  let mut field_declaration_list: Vec<Stmt> = vec![];
  let mut field_match_list: Vec<Arm> = vec![];
  let mut field_unwrap_list: Vec<TokenStream> = vec![];
  let mut field_init_list: Vec<TokenStream> = vec![];
  let mut child_ser_match_list: Vec<Arm> = vec![];
  let mut child_de_match_list: Vec<Arm> = vec![];

  let xmlns_literal: LitByteStr =
    parse_str(&format!("b\"xmlns:{}\"", schema_namespace.prefix)).unwrap();

  field_match_list.push(
    parse_str(
      &quote! {
        #xmlns_literal => with_xmlns = true,
      }
      .to_string(),
    )
    .unwrap(),
  );

  for attr in &t.attributes {
    let attr_name_ident: Ident = if attr.property_name.is_empty() {
      parse_str(&escape_snake_case(attr.q_name.to_snake_case())).unwrap()
    } else {
      parse_str(&escape_snake_case(attr.property_name.to_snake_case())).unwrap()
    };

    field_declaration_list.push(
      parse_str(
        &quote! {
          let mut #attr_name_ident = None;
        }
        .to_string(),
      )
      .unwrap(),
    );

    field_match_list.push(gen_field_match_arm(attr, context));

    let mut required = false;

    for validator in &attr.validators {
      if validator.name == "RequiredValidator" {
        required = true;
      }
    }

    if required {
      field_unwrap_list.push(quote! {
        let #attr_name_ident = #attr_name_ident.ok_or_else(|| super::deserializers::DeError::UnknownError)?;
      })
    }

    field_init_list.push(quote! {
      #attr_name_ident,
    })
  }

  if !t.children.is_empty() {
    field_declaration_list.push(
      parse_str(
        &quote! {
          let mut children = vec![];
        }
        .to_string(),
      )
      .unwrap(),
    );

    field_init_list.push(quote! {
      children,
    })
  }

  let scheme_mod = context
    .prefix_schema_mod_map
    .get(schema_namespace.prefix.as_str())
    .ok_or(format!("{:?}", schema_namespace.prefix))
    .unwrap();

  let child_choice_enum_type: Type = parse_str(&format!(
    "crate::schemas::{}::{}ChildChoice",
    scheme_mod,
    t.class_name.to_upper_camel_case()
  ))
  .unwrap();

  let mut child_variant_name_set: HashSet<String> = HashSet::new();

  for child in &t.children {
    if !child_variant_name_set.contains(child.name.as_str()) {
      let (ser_arm, de_arm) = gen_child_match_arm(child, &child_choice_enum_type, context);

      child_ser_match_list.push(ser_arm);

      child_de_match_list.push(de_arm);

      child_variant_name_set.insert(child.name.clone());
    }
  }

  let child_match_arm: TokenStream = if t.children.is_empty() {
    quote! {}
  } else {
    quote! {
      quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e) => {
        if with_xmlns {
          match e.name().as_ref() {
            #( #child_ser_match_list )*
            _ => Err(super::deserializers::DeError::UnknownError)?,
          }
        } else {
          match e.name().local_name().as_ref() {
            #( #child_de_match_list )*
            _ => Err(super::deserializers::DeError::UnknownError)?,
          }
        }
      }
    }
  };

  let token_stream = quote! {
    pub fn deserialize_self<'de, R: super::deserializers::XmlReader<'de>>(
      xml_reader: &mut R,
      with_xmlns: bool,
    ) -> Result<Self, super::deserializers::DeError> {
      let mut with_xmlns = with_xmlns;

      if let quick_xml::events::Event::Start(e) = xml_reader.next()? {
        if e.name().local_name().as_ref() != #rename_de_literal {
          Err(super::deserializers::DeError::UnknownError)?;
        }

        #( #field_declaration_list )*

        for attr in e.attributes() {
          let attr = attr?;

          match attr.key.as_ref() {
            #( #field_match_list )*
            _ => (),
          }
        }

        if with_xmlns {
          if e.name().as_ref() != #rename_ser_literal {
            Err(super::deserializers::DeError::UnknownError)?;
          }
        } else if e.name().local_name().as_ref() != #rename_de_literal {
          Err(super::deserializers::DeError::UnknownError)?;
        }

        loop {
          let peek_event = xml_reader.peek()?;

          match peek_event {
            #child_match_arm
            quick_xml::events::Event::End(e) => {
              if with_xmlns {
                if e.name().as_ref() == #rename_ser_literal {
                  xml_reader.next()?;

                  break;
                }
              } else if e.name().local_name().as_ref() == #rename_de_literal {
                xml_reader.next()?;

                break;
              }
            }
            quick_xml::events::Event::Eof => Err(super::deserializers::DeError::UnknownError)?,
            _ => (),
          }
        }

        #( #field_unwrap_list )*

        Ok(Self {
          #( #field_init_list )*
        })
      } else {
        Err(super::deserializers::DeError::UnknownError)?
      }
    }
  };

  parse_str(&token_stream.to_string()).unwrap()
}

fn gen_derived_fn(_t: &OpenXmlSchemaType) -> ItemFn {
  let token_stream = quote! {
    pub fn deserialize_self<'de, R: super::deserializers::XmlReader<'de>>(
      xml_reader: &mut R,
      with_xmlns: bool,
    ) -> Result<Self, super::deserializers::DeError> {
      Err(super::deserializers::DeError::UnknownError)?
    }
  };

  parse_str(&token_stream.to_string()).unwrap()
}

fn gen_field_match_arm(attr: &OpenXmlSchemaTypeAttribute, context: &GenContext) -> Arm {
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

  let attr_rename_ser_literal: LitByteStr =
    parse_str(&format!("b\"{}\"", attr_rename_ser_str)).unwrap();

  let token_stream: TokenStream = if attr.r#type.starts_with("ListValue<") {
    quote! {
      #attr_rename_ser_literal => {
        #attr_name_ident = Some(
          attr
            .decode_and_unescape_value(xml_reader.decoder())?
            .to_string(),
        )
      }
    }
  } else if attr.r#type.starts_with("EnumValue<") {
    let type_list: Vec<&str> = attr.r#type.split('.').collect();

    let enum_name = type_list
      .last()
      .ok_or(format!("{:?}", attr.r#type))
      .unwrap();
    let enum_name = &enum_name[0..enum_name.len() - 1];

    let e = context
      .enum_name_enum_map
      .get(enum_name)
      .ok_or(format!("{:?}", enum_name))
      .unwrap();

    let e_namespace = context
      .enum_type_namespace_map
      .get(e.r#type.as_str())
      .ok_or(format!("{:?}", e.r#type))
      .unwrap();

    let scheme_mod = context
      .prefix_schema_mod_map
      .get(e_namespace.prefix.as_str())
      .ok_or(format!("{:?}", e_namespace.prefix))
      .unwrap();

    let e_type: Type = parse_str(&format!(
      "crate::schemas::{}::{}",
      scheme_mod,
      e.name.to_upper_camel_case()
    ))
    .unwrap();

    quote! {
      #attr_rename_ser_literal => {
        #attr_name_ident = Some(#e_type::from_str(
          &attr.decode_and_unescape_value(xml_reader.decoder())?,
        )?)
      }
    }
  } else {
    match attr.r#type.as_str() {
      "Base64BinaryValue" | "DateTimeValue" | "DecimalValue" | "HexBinaryValue"
      | "IntegerValue" | "SByteValue" | "StringValue" => quote! {
        #attr_rename_ser_literal => {
          #attr_name_ident = Some(
            attr
              .decode_and_unescape_value(xml_reader.decoder())?
              .to_string(),
          )
        }
      },
      "BooleanValue" | "OnOffValue" | "TrueFalseBlankValue" | "TrueFalseValue" => quote! {
        #attr_rename_ser_literal => {
          #attr_name_ident = Some(
            match attr
              .decode_and_unescape_value(xml_reader.decoder())?
              .as_ref()
            {
              "true" | "1" | "True" | "TRUE" | "t" | "Yes" | "YES" | "yes" | "y" => true,
              "false" | "0" | "False" | "FALSE" | "f" | "No" | "NO" | "no" | "n" | "" => false,
              _ => Err(super::deserializers::DeError::UnknownError)?,
            }
          )
        }
      },
      "ByteValue" | "Int16Value" | "Int32Value" | "Int64Value" | "UInt16Value" | "UInt32Value"
      | "UInt64Value" | "DoubleValue" | "SingleValue" => {
        let e_type: Type =
          parse_str(&format!("crate::schemas::simple_type::{}", &attr.r#type)).unwrap();

        quote! {
          #attr_rename_ser_literal => {
            #attr_name_ident = Some(
              attr
                .decode_and_unescape_value(xml_reader.decoder())?
                .parse::<#e_type>()?,
            )
          }
        }
      }
      _ => panic!("{}", attr.r#type),
    }
  };

  parse_str(&token_stream.to_string()).unwrap()
}

fn gen_child_match_arm(
  child: &OpenXmlSchemaTypeChild,
  child_choice_enum_ident: &Type,
  context: &GenContext,
) -> (Arm, Arm) {
  let child_type = context
    .type_name_type_map
    .get(child.name.as_str())
    .ok_or(format!("{:?}", child.name))
    .unwrap();

  let child_namespace = context
    .type_name_namespace_map
    .get(child.name.as_str())
    .ok_or(format!("{:?}", child.name))
    .unwrap();

  let child_name_list: Vec<&str> = child.name.split('/').collect();

  let child_rename_ser_str = child_name_list
    .last()
    .ok_or(format!("{:?}", child.name))
    .unwrap();

  let child_rename_list: Vec<&str> = child_rename_ser_str.split(':').collect();

  let child_rename_de_str = child_rename_list
    .last()
    .ok_or(format!("{:?}", child.name))
    .unwrap();

  let child_rename_ser_literal: LitByteStr =
    parse_str(&format!("b\"{}\"", child_rename_ser_str)).unwrap();

  let child_rename_de_literal: LitByteStr =
    parse_str(&format!("b\"{}\"", child_rename_de_str)).unwrap();

  let child_variant_name_ident: Ident =
    parse_str(&child_rename_ser_str.to_upper_camel_case()).unwrap();

  let scheme_mod = context
    .prefix_schema_mod_map
    .get(child_namespace.prefix.as_str())
    .ok_or(format!("{:?}", child_namespace.prefix))
    .unwrap();

  let child_variant_type: Type = parse_str(&format!(
    "crate::schemas::{}::{}",
    scheme_mod,
    child_type.class_name.to_upper_camel_case()
  ))
  .unwrap();

  let ser_token_stream: TokenStream = quote! {
    #child_rename_ser_literal => {
      children.push(#child_choice_enum_ident::#child_variant_name_ident(std::boxed::Box::new(
        #child_variant_type::deserialize_self(xml_reader, with_xmlns)?,
      )));
    }
  };

  let de_token_stream: TokenStream = quote! {
    #child_rename_de_literal => {
      children.push(#child_choice_enum_ident::#child_variant_name_ident(std::boxed::Box::new(
        #child_variant_type::deserialize_self(xml_reader, with_xmlns)?,
      )));
    }
  };

  (
    parse_str(&ser_token_stream.to_string()).unwrap(),
    parse_str(&de_token_stream.to_string()).unwrap(),
  )
}
