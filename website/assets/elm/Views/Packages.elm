module Views.Packages exposing (packagesView)

--import Views.Common exposing (..)
import Views.Index exposing (headerView)
import Html exposing (..)
import Html.Attributes exposing (..)
--import Html.Events exposing (..)
--import Html.Keyed exposing (..)
import Messages exposing (..)
import Model exposing (..)


packagesView : Model -> Html Msg
packagesView model =
    div [ class "packages" ] [
        headerView model,
        hr [ class "header" ] [],
        listView model
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
