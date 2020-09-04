module Page.Footer exposing (view)

import Page.FooterCss exposing (divider90, links, aFooter)
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (css, href)
import Messages exposing (..)
import Route
import Model exposing (Model)


view : Model -> Html Msg
view model =
    footer []
        [ hr [ css [ divider90 ] ] []
        , div [ css [ links model ]
              ]
              [ aFooter
                  [ href "mailto:support@poac.pm?subject=[Feedback]" ]
                  [ text "Feedback" ]
              , aFooter
                  [ href "https://github.com/poacpm" ]
                  [ text "GitHub" ]
              , aFooter
                  [ Route.href Route.Policy ]
                  [ text "Policies" ]
--              , a [ href <| Routing.pathFor SponsorsRoute ]
--                  [ text "Sponsors" ]
              ]
        ]
