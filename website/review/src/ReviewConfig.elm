module ReviewConfig exposing (config)

{-| Do not rename the ReviewConfig module or the config function, because
`elm-review` will look for these.

To add packages that contain rules, add them to this review project using

    `elm install author/packagename`

when inside the directory containing this file.

-}

import NoAlways
import NoBooleanCase
import NoDebug.Log
import NoDebug.TodoOrToString
import NoDuplicatePorts
import NoEmptyText
import NoFloatIds
import NoMissingSubscriptionsCall
import NoMissingTypeAnnotationInLetIn
import NoRecursiveUpdate
import NoRedundantConcat
import NoRedundantCons
import NoTypeAliasConstructorCall
import NoUnused.CustomTypeConstructors
import NoUnused.Dependencies
import NoUnused.Exports
import NoUnused.Modules
import NoUnused.Parameters
import NoUnused.Patterns
import NoUnused.Variables
import NoUselessSubscriptions
import Review.Rule exposing (Rule)


config : List Rule
config =
    [ NoAlways.rule
    , NoBooleanCase.rule
    , NoDebug.Log.rule
    , NoDebug.TodoOrToString.rule
    , NoDuplicatePorts.rule
    , NoEmptyText.rule
    , NoFloatIds.rule
    , NoMissingSubscriptionsCall.rule
    , NoMissingTypeAnnotationInLetIn.rule
    , NoRecursiveUpdate.rule
    , NoRedundantConcat.rule
    , NoRedundantCons.rule
    , NoTypeAliasConstructorCall.rule
    , NoUnused.CustomTypeConstructors.rule []
    , NoUnused.Dependencies.rule
    , NoUnused.Exports.rule
    , NoUnused.Modules.rule
    , NoUnused.Parameters.rule
    , NoUnused.Patterns.rule
    , NoUnused.Variables.rule
    , NoUselessSubscriptions.rule
    ]
