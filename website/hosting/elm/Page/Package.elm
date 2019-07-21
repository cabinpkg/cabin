module Page.Package exposing (view)

import Assets
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Route
import Page.NotFound as NotFound
import Markdown



view : Model -> Html Msg
view model =
    main_
        [ class "packages" ]
        [ detailView model ]


detailView : Model -> Html Msg
detailView model =
    case model.package of
        Success package ->
            detailMainView model package

        Requesting ->
            div [ class "spinner" ]
                [ Assets.spinner ]

        _ ->
            NotFound.view


detailMainView : Model -> PackageMetadata -> Html Msg
detailMainView model package =
    div [ class "container" ]
        [ span [ class "package-title" ]
            [ a [ Route.href (Route.OwnPackages package.owner) ]
                [ text package.owner ]
            , text " / "
            , a [ Route.href (Route.PackageVersions package.owner package.repo) ]
                [ text package.repo ]
            ]
        , span [ class "hit-version" ]
            [ text package.version
            ]
        , div [ class "hit-description" ]
            [ text package.description
            ]
        , div [ class "cpp-version" ]
            [ text <| "C++ version: " ++ String.fromInt package.cppVersion
            ]
        , div [ class "package-root" ]
              [
              case package.readme of
                  Just readme ->
                      -- https://stackoverflow.com/questions/48755746/new-line-command-n-not-working-with-firebase-firestore-database-strings
                      Markdown.toHtml [ class "readme" ] (String.replace "\\n" "\n" readme)

                  Nothing ->
                      div []
                          [ text <| "Unable to find a readme for " ++ package.owner ++ "/" ++ package.repo ++ ": " ++ package.version ]
              ]
        ]
