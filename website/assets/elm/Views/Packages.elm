module Views.Packages exposing (view)

import Views.Header as Header
import Views.NotFound as NotFound
import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> String -> Html Msg
view model name =
    selectListOrDetail model name

mainView : Model -> Html Msg -> Html Msg
mainView model element =
    div [ class "packages" ] [
        Header.view model,
        element
    ]

selectListOrDetail : Model -> String -> Html Msg
selectListOrDetail model name =
    if String.isEmpty name then
        listView model
    else
        detailView model


listView : Model -> Html Msg
listView model =
    case model.listPackages of
        Success listPackages ->
            mainView model (div [ class "container" ] (List.map packageView listPackages))
        Requesting ->
            mainView model (a [] [ text "Loading..." ])
        _ ->
            NotFound.view model

packageView : Package -> Html Msg
packageView package =
    div [ class "list-item" ] [
        a [ class "packname"
          , href ("/packages/" ++ package.name) -- TODO: NavigateTo
        ] [ text package.name ],
        span [ class "version" ] [ text package.version ],
        p [ class "description" ] [ text package.description ]
    ]


detailView : Model -> Html Msg
detailView model =
    case model.detailedPackage of
        Success detailedPackage ->
            mainView model (detailMainView detailedPackage)
        Requesting ->
            mainView model (a [] [ text "Loading..." ])
        _ ->
            NotFound.view model

detailMainView : DetailedPackage -> Html Msg
detailMainView detailedPackage =
    div [ class "container" ] [
        h2 [ class "package-title" ] [
            text detailedPackage.name
        ]
        , div [ class "description" ] [
            text detailedPackage.description
        ]
    ]
