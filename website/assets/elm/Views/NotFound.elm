module Views.NotFound exposing (notFoundView)

import Views.Common exposing (warningMessage, backToHomeLink)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Html.Keyed exposing (..)
import Messages exposing (..)
import Model exposing (..)


notFoundView : Html Msg
notFoundView =
    warningMessage
        "fa fa-meh-o fa-stack-2x"
        "Page not found"
        backToHomeLink
