module Views.Settings.Packages exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Views.Svgs as Svgs


view : Model -> Html Msg
view model =
    case model.signinUser of
        Success _ ->
            case model.listPackages of
                Success listPackages ->
                    div [ class "content" ]
                        [ h2 [] [ text "Packages" ]
                        , div [ class "list" ]
                              (List.map packageView listPackages) -- TODO: listPackages.size() == zero
                        ]
                Requesting ->
                    div [ class "content" ]
                        [ h2 [] [ text "Packages" ]
                        , div [ class "spinner" ]
                              [ Svgs.spinner ]
                        ]
                _ ->
                    div [ class "content" ]
                        [ h2 [] [ text "Packages" ]
                        , a [] [ text "Please signin" ] -- FIXME
                        ]
        _ ->
            div [ class "content" ]
                [ h2 [] [ text "Packages" ]
                , a [] [ text "Please signin" ]
                ]

packageView : Package -> Html Msg
packageView package =
    div [ class "list-item", style [("justify-content", "space-between"), ("display", "flex")] ]
        [ a [ class "packname"
            , href ("/packages/" ++ package.name)
            ] [ text package.name ]
        , span [ class "version" ] [ text package.version ]
        , a [ class "common-button background-color-red"
            , onClick <| DeletePackage package.name package.version
            ] [ text "Delete" ]
        ]
