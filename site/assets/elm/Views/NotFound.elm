module Views.NotFound exposing (view)

import Messages exposing (..)
import Html exposing (..)
import Html.Attributes exposing (..)


view : Html Msg
view =
    div [ class "notfound" ]
      [ h1 [] [ text "404" ]
      , h2 [] [ text "Page not found" ]
      ]
