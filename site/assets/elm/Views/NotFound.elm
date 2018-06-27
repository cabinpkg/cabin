module Views.NotFound exposing (view)

--import Views.Common exposing (..)
import Views.Header as Header
import Html exposing (..)
import Html.Attributes exposing (..)
--import Html.Events exposing (..)
--import Html.Keyed exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
    div [ class "notfound" ] [
        Header.view model,
        h1 [] [ text "404" ],
        h2 [] [ text "Page not found" ]
    ]
