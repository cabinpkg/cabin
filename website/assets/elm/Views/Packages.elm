module Views.Packages exposing (view)

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
    div [ class "packages" ] [
        Header.view model,
        h2 [ style [ ("color", "red") ] ] [
            text "Sorry...",
            br [] [],
            text "Packages is not yet implemented.",
            br [] [],
            text "Please wait for it..."
        ]
--        hr [ class "header" ] [],
--        listView model
    ]

listView : Model -> Html Msg
listView model =
    div [] [
        span [ class "num" ] [ text "100 Packages Found" ],
        div [ class "sort" ] [
            select [ name "sort" ] [
                option [ value "Name" ] [ text "Name" ],
                option [ value "Popularity" ] [ text "Popularity" ],
                option [ value "Downloads" ] [ text "Downloads" ]
            ]
        ]
    ]
