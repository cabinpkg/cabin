module Views.NotFound exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)


view : Html Msg
view =
    main_ [ class "notfound" ]
        [ h1 [] [ text "404" ]
        , h2 [] [ text "Page not found" ]
        ]
