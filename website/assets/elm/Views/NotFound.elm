module Views.NotFound exposing (notFoundView)

import Views.Common exposing (..)
import Views.Index exposing (headerView)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Html.Keyed exposing (..)
import Messages exposing (..)
import Model exposing (..)


notFoundView : Html Msg
notFoundView =
    div [ class "notfound" ]
        [ headerView,
--          div [ class "icon" ] [ i [ class "fa fa-meh" ] [] ],
          h1 [] [ text "404" ],
          h2 [] [ text "Page not found" ] ]
