use chrono::prelude::*;
use connector::Query;
use prisma_models::{GraphqlId, PrismaValue, ModelRef};
use std::{
    collections::BTreeMap,
    result::Result,
    sync::Arc
};
use uuid::Uuid;
use crate::{
    query_document::Selection,
    query_document::{
        QueryValue,
        Operation,
        QueryDocument,
        ReadOperation
    },
    Argument,
    CoreError,
    CoreResult,
    EnumType,
    Field,
    InputObjectTypeStrongRef,
    InputType,
    IntoArc,
    QuerySchemaRef,
    QueryValidationError,
    ScalarType,
    OperationTag,
    OutputType,
    FieldRef,
    ObjectTypeStrongRef,
    query_builders::{
        QueryBuilderResult,
        read_new::ReadOneRecordBuilder,
        ReadQueryBuilder,
        ParsedObject,
        ParsedField,
        ParsedArgument,
        ParsedInputValue
    }
};

pub struct QueryBuilder {
    pub query_schema: QuerySchemaRef,
}

// Todo:
// - Use error collections instead of letting first error win.
// - Check if empty selection set already fails at the parser level
// - UUID ids are not encoded in any useful way in the schema.
// - Should the required field injection be done here?
// - Alias handling in query names
impl QueryBuilder {
    pub fn new(query_schema: QuerySchemaRef) -> Self {
        QueryBuilder { query_schema }
    }

    /// Builds queries from a query document.
    pub fn build(self, query_doc: QueryDocument) -> CoreResult<Vec<Query>> {
        query_doc
            .operations
            .into_iter()
            .map(|op| self.map_operation(op).map_err(|err| err.into()))
            .collect::<Vec<QueryBuilderResult<Vec<Query>>>>()
            .into_iter().collect::<QueryBuilderResult<Vec<Vec<Query>>>>()
            .map(|vec| vec.into_iter().flatten().collect()).map_err(|err| err.into())
    }

    /// Maps an operation to a query.
    fn map_operation(&self, operation: Operation) -> QueryBuilderResult<Vec<Query>> {
        match operation {
            Operation::Read(read_op) => self.map_read(read_op),
            Operation::Write(write_op) => unimplemented!(),
        }
    }

    /// Maps a read operation to a query.
    fn map_read(&self, read_op: ReadOperation) -> QueryBuilderResult<Vec<Query>> {
        let query_object = self.query_schema.query();
        let parsed = self.parse_object(&read_op.selections, &query_object)?;

        // Special treatment on read root: all fields map to a model operation.
        // This means: Find matching schema field (which is a bit redundant here,
        // because it was done during object parsing already).
        // Then, for each field on the query object: build a query.
        parsed.fields.into_iter().map(|parsed_field| {
            let field = query_object.find_field(parsed_field.name.clone()).expect("Expected validation to guarantee existing field on Query object.");
            let field_operation = field.operation.as_ref().expect("Expected Query object fields to always have an associated operation.");

            let builder = match field_operation.operation {
                OperationTag::FindOne => ReadOneRecordBuilder::new(parsed_field, Arc::clone(&field_operation.model)),
                OperationTag::FindMany => unimplemented!(),
                _ => unreachable!(), // Only read one / many is possible on the root.
            };

            builder.build().map(|query| Query::Read(query))
        }).collect()
    }

    /// Parses and validates a set of selections against a schema (output) object.
    fn parse_object(&self, selections: &Vec<Selection>, object: &ObjectTypeStrongRef) -> QueryBuilderResult<ParsedObject> {
        if selections.len() == 0 {
            return Err(QueryValidationError::ObjectValidationError {
                object_name: object.name.clone(),
                inner: Box::new(QueryValidationError::AtLeastOneSelectionError)
            })
        }

        selections
            .into_iter()
            .map(|selection| {
                let parsed_field = match object.find_field(selection.name.as_ref()) {
                    Some(ref field) => self.parse_field(selection, field),
                    None => Err(QueryValidationError::FieldValidationError {
                        field_name: selection.name.clone(),
                        inner: Box::new(QueryValidationError::FieldNotFoundError),
                    }),
                };

                parsed_field.map_err(|err| QueryValidationError::ObjectValidationError {
                    object_name: object.name.clone(),
                    inner: Box::new(err),
                })
            })
            .collect::<QueryBuilderResult<Vec<ParsedField>>>().map(|fields| { ParsedObject { fields } })
    }

    /// Parses and validates a selection against a schema (output) field.
    fn parse_field(&self, selection: &Selection, schema_field: &FieldRef) -> QueryBuilderResult<ParsedField> {
        // Parse and validate all provided arguments for the field
        self.parse_arguments(schema_field, &selection.arguments).and_then(|arguments| {
            // If the output type of the field is an object type of any form, validate the sub selection as well.
            let sub_selections = schema_field.field_type.as_object_type().map(|obj| self.parse_object(&selection.sub_selections, &obj));
            let sub_selections = match sub_selections {
                Some(sub) => Some(sub?),
                None => None
            };

            Ok(ParsedField {
                name: selection.name.clone(),
                alias: selection.alias.clone(),
                arguments,
                sub_selections,
            })
        }).map_err(|err| {
            QueryValidationError::FieldValidationError {
                field_name: schema_field.name.clone(),
                inner: Box::new(err),
            }
        })
    }

    /// Parses and validates selection arguments against a schema defined field.
    fn parse_arguments(&self, schema_field: &FieldRef, given_arguments: &Vec<(String, QueryValue)>) -> QueryBuilderResult<Vec<ParsedArgument>> {
        schema_field
            .arguments
            .iter()
            .filter_map(|schema_arg| {
                // Match schema field to a field in the incoming document
                let selection_arg: Option<(String, QueryValue)> = given_arguments
                    .iter()
                    .find(|given_argument| given_argument.0 == schema_arg.name)
                    .cloned();

                // If the arg can be found, parse the provided query value into a list / object / PrismaValue.
                //
                // If the arg can _not_ be found, pretend the arg was provided with a Null.
                // Run the validation against the Null value to check if it needs to be provided, but disregard the result if it succeeded.
                let (selection_arg, retain) = match selection_arg {
                    Some(arg) => (arg, true),
                    None => ((schema_arg.name.clone(), QueryValue::Null), false),
                };

                let result = self
                    .parse_input_value(selection_arg.1, &schema_arg.argument_type)
                    .map(|value| ParsedArgument { name: schema_arg.name.clone(), value } )
                    .map_err(|err| QueryValidationError::ArgumentValidationError {
                        argument: schema_arg.name.clone(),
                        inner: Box::new(err),
                    });

                if result.is_err() || retain {
                    Some(result)
                } else {
                    None
                }
            })
            .collect::<Vec<QueryBuilderResult<ParsedArgument>>>().into_iter().collect()
    }

    /// Parses and validates a QueryValue against an InputType, recursively.
    #[rustfmt::skip]
    fn parse_input_value(&self, value: QueryValue, input_type: &InputType) -> QueryBuilderResult<ParsedInputValue> {
        dbg!(&value);
        dbg!(input_type); // todo figure out what is up with enums

        match (&value, input_type) {
            // Handle null inputs
            (QueryValue::Null, InputType::Opt(_))       => Ok(ParsedInputValue::Single(PrismaValue::Null)),
            (_, InputType::Opt(ref inner))                    => self.parse_input_value(value, inner),

            // The optional handling above guarantees that if we hit a Null here, a required value is missing.
            (QueryValue::Null, _)                           => Err(QueryValidationError::RequiredValueNotSetError),

            // Scalar and enum handling.
            (_, InputType::Scalar(scalar))                => self.parse_scalar(value, &scalar).map(|pv| ParsedInputValue::Single(pv)),
//            (QueryValue::Enum(_), InputType::Enum(et))      => self.parse_scalar(val, &ScalarType::Enum(Arc::clone(et))).map(|pv| ParsedInputValue::Single(pv)), // todo

            // List and object handling.
            // This is the only mismatch special case we need to handle separately (objects in scalar lists), because of 1-element coercion that is done below.
            (QueryValue::Object(_), InputType::List(_))     => Err(QueryValidationError::ValueTypeMismatchError { have: value, want: input_type.clone() }),
            (QueryValue::List(values), InputType::List(l))  => self.parse_list(*values, &l).map(|pv| ParsedInputValue::Single(pv)),
            (_, InputType::List(l))                       => self.parse_list(vec![value], &l).map(|pv| ParsedInputValue::Single(pv)),
            (QueryValue::Object(o), InputType::Object(obj)) => self.parse_input_object(*o, obj.into_arc()).map(|btree| ParsedInputValue::Map(btree)),
            (val, input_type)                                       => Err(QueryValidationError::ValueTypeMismatchError { have: value, want: input_type.clone() }),
        }
    }

    /// Attempts to parse given query value into a concrete PrismaValue based on given scalar type.
    #[rustfmt::skip]
    fn parse_scalar(&self, value: QueryValue, scalar_type: &ScalarType) -> QueryBuilderResult<PrismaValue> {
        match (value, scalar_type.clone()) {
            (QueryValue::Null, _)                         => Ok(PrismaValue::Null),
            (QueryValue::String(s), ScalarType::String)   => Ok(PrismaValue::String(s)),
            (QueryValue::String(s), ScalarType::DateTime) => Self::parse_datetime(s.as_str()).map(|dt| PrismaValue::DateTime(dt)),
            (QueryValue::String(s), ScalarType::Json)     => Self::parse_json(s.as_str()).map(|j| PrismaValue::Json(j)),
            (QueryValue::String(s), ScalarType::UUID)     => Self::parse_uuid(s.as_str()).map(|u| PrismaValue::Uuid(u)),
            (QueryValue::Int(i), ScalarType::Int)         => Ok(PrismaValue::Int(i)),
            (QueryValue::Float(f), ScalarType::Float)     => Ok(PrismaValue::Float(f)),
            (QueryValue::Boolean(b), ScalarType::Boolean) => Ok(PrismaValue::Boolean(b)),
            (QueryValue::Enum(e), ScalarType::Enum(et))   => match et.value_for(e.as_str()).and_then(|val| val.as_string()) {
                                                                Some(val) => Ok(PrismaValue::Enum(val)),
                                                                None => Err(QueryValidationError::ValueParseError(format!("Enum value '{}' is invalid for enum type {}.", e, et.name)))
                                                             },

            // Possible ID combinations TODO UUID ids are not encoded in any useful way in the schema.
            (QueryValue::String(s), ScalarType::ID)       => Self::parse_uuid(s.as_str()).map(|u| PrismaValue::Uuid(u)).or_else(|_| Ok(PrismaValue::String(s))),
            (QueryValue::Int(i), ScalarType::ID)          => Ok(PrismaValue::GraphqlId(GraphqlId::Int(i as usize))),

            // Remainder of combinations is invalid
            (qv, st)                                      => Err(QueryValidationError::ValueTypeMismatchError { have: qv, want: InputType::Scalar(scalar_type.clone()) }),
        }
    }

    fn parse_datetime(s: &str) -> QueryBuilderResult<DateTime<Utc>> {
        let fmt = "%Y-%m-%dT%H:%M:%S%.3f";
        Utc.datetime_from_str(s.trim_end_matches("Z"), fmt)
            .map(|dt| DateTime::<Utc>::from_utc(dt.naive_utc(), Utc))
            .map_err(|err| {
                QueryValidationError::ValueParseError(format!(
                    "Invalid DateTime: {} DateTime must adhere to format: %Y-%m-%dT%H:%M:%S%.3f",
                    err
                ))
            })
    }

    fn parse_json(s: &str) -> QueryBuilderResult<serde_json::Value> {
        serde_json::from_str(s).map_err(|err| QueryValidationError::ValueParseError(format!("Invalid json: {}", err)))
    }

    fn parse_uuid(s: &str) -> QueryBuilderResult<Uuid> {
        Uuid::parse_str(s).map_err(|err| QueryValidationError::ValueParseError(format!("Invalid UUID: {}", err)))
    }

    fn parse_list(&self, values: Vec<QueryValue>, value_type: &InputType) -> QueryBuilderResult<PrismaValue> {
        let values: Vec<ParsedInputValue> = values
            .into_iter()
            .map(|val| self.parse_input_value(val, value_type))
            .collect::<QueryBuilderResult<Vec<ParsedInputValue>>>()?;

        let values: Vec<PrismaValue> = values
            .into_iter()
            .map(|val| match val {
                ParsedInputValue::Single(inner) => inner,
                _ => unreachable!(), // Objects represent relations, which are handled separately and can't occur for scalar lists.
            })
            .collect();

        Ok(PrismaValue::List(Some(values)))
    }

    /// Parses and validates an input object recursively.
    fn parse_input_object(
        &self,
        mut object: BTreeMap<String, QueryValue>,
        schema_object: InputObjectTypeStrongRef,
    ) -> QueryBuilderResult<BTreeMap<String, ParsedInputValue>> {
        schema_object
            .get_fields()
            .iter()
            .map(|field| {
                // todo this doesn't work
                // Find field in the passed object

                match object.remove(&field.name) {

//                match object.iter().find(|(k, v)| *k == &field.name) {
                    Some(v) => self
                        .parse_input_value(v, &field.field_type)
                        .map(|parsed| (field.name.clone(), parsed)),

                    None => {
                        // Find default value and use that one if the field can't be found.
                        match field.default_value.clone().map(|def| (&field.name, def)) {
                            Some((k, v)) => Ok((k.clone(), ParsedInputValue::Single(v))),
                            None => Err(QueryValidationError::FieldNotFoundError),
                        }
                    }
                }
            })
            .collect::<QueryBuilderResult<Vec<(String, ParsedInputValue)>>>()
            .map(|tuples| tuples.into_iter().collect())
    }
}
