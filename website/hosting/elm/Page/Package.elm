module Page.Package exposing (view)

import Assets
import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Route
import Page.NotFound as NotFound
import Markdown



view : Model -> Html Msg
view model =
    main_
        [ class "package" ]
        [ detailView model ]


detailView : Model -> Html Msg
detailView model =
    case model.package of
        Success package ->
            detailMainView package

        Requesting ->
            div [ class "spinner" ]
                [ Assets.spinner ]

        _ ->
            NotFound.view


detailMainView : PackageMetadata -> Html Msg
detailMainView package =
    let
        name = package.owner ++ "/" ++ package.repo
    in
    div [ class "center" ]
        [ a
              [ Route.href (Route.OwnPackages package.owner) ]
              [ text package.owner ]
        , span
              [ class "spacey-char" ]
              [ text "/" ]
        , a
              [ Route.href (Route.PackageVersions package.owner package.repo) ]
              [ text package.repo ]
        , span
              [ class "spacey-char" ]
              [ text "/" ]
        , a
              [ Route.href (Route.Package package.owner package.repo package.version) ]
              [ text package.version ]
        , a
              [ class "tag"
              , href <| "/packages?dFR%5Bcpp_version%5D%5B0%5D=" ++ (String.fromInt package.cppVersion)
              ]
              [ text <| "c++" ++ String.fromInt package.cppVersion ]
        , a
              [ class "tag"
              , href <| "/packages?dFR%5Bpackage_type%5D%5B0%5D=" ++ package.packageType
              ]
              [ text package.packageType ]
        , div [ class "description" ]
              [ text package.description
              ]
        , a
              [ class "link"
              , href ("https://github.com/" ++ name ++ "/tree/" ++ package.version)
              ]
              [ text "Browse Source" ]
        , if String.isEmpty package.readme then
              div []
                  [ text <| "Unable to find a readme for " ++ name ++ ": " ++ package.version ]
          else
              -- https://stackoverflow.com/a/48874269
              Markdown.toHtml [ class "readme" ] (String.replace "\\n" "\n" package.readme)
        ]
