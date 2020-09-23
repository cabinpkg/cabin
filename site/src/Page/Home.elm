module Page.Home exposing (view)

import Css exposing (..)
import Css.Global as Global exposing (children, everything)
import Css.Media exposing (withMediaQuery)
import Html.Parser
import Html.Parser.Util
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (alt, autocomplete, css, for, href, id, placeholder, src, type_)
import Html.Styled.Events exposing (..)
import Html.Styled.Extra exposing (viewIf)
import Json.Decode as Json
import Messages exposing (Msg(..))
import Model exposing (..)
import Route
import Style.Autoprefixer exposing (..)
import Style.Extra exposing (ifMobile, unselectable)


homeViewWidth : Int -> Style
homeViewWidth currentWidth =
    if currentWidth > 1000 then
        property "width" "calc(85% - 2 * (100vw - 1000px) / 200)"

    else if currentWidth > 1200 then
        width (pct 80)

    else
        width (vw 90)


view : Model -> Html Msg
view model =
    main_
        [ css
            [ withMediaQuery
                [ "(max-width: 1150px)" ]
                [ children
                    [ everything
                        [ width (pct 90) |> important -- TODO: this should be deleted
                        ]
                    ]
                ]
            , homeViewWidth model.width
            , marginRight auto
            , marginLeft auto
            , textAlign center
            , fontFamilies [ "montserrat", .value sansSerif ]
            ]
        ]
        [ img
            [ css
                [ property "pointer-events" "none"
                , width (vw 60)
                ]
            , src "/images/terminal.svg"
            , alt "terminal demonstration"
            ]
            []
        , phraseView model
        , getStartedView model
        , section model
        ]


headlineTextStyle : Style
headlineTextStyle =
    Css.batch
        [ marginTop (px 15)
        , marginBottom (px 15)
        , lineHeight (num 1.5)
        , fontWeight normal
        ]


phraseView : Model -> Html Msg
phraseView model =
    div
        [ css
            [ width (px 800)
            , marginTop (px 50)
            , marginRight auto
            , marginLeft auto
            , fontStyle normal
            ]
        ]
        [ h1 []
            [ text "Package Manager for C++ Developers"
            ]
        , h2 [ css [ headlineTextStyle ] ]
            [ text "Poac is a C++ package manager which is for open source software."
            ]
        , h3 [ css [ headlineTextStyle ] ]
            [ text """Easy to introduce to your projects;
                   you can use packages intuitively."""
            ]
        , searchBox model
        ]


algoliaSearchInputStyle : Model -> Style
algoliaSearchInputStyle model =
    Css.batch
        [ ifMobile model.width (width (vw 80)) (width (px 300))
        , height (px 40)
        , marginTop (px 20)
        , padding (px 12)
        , border3 (px 2) solid (hex "e4e4e4")
        , borderRadius (px 4)
        , fontWeight (int 600)
        , fontSize (px <| ifMobile model.width 16 13)
        , fontStyle normal
        , model.theme.color
        , model.theme.backgroundColor
        , legacyTransition ".2s"
        , legacyBoxShadow "0 3px 15px 0 #b9b9b9"
        , legacyBoxSizing "border-box"
        , appearance "none"
        , outline zero
        , focus
            [ outline zero
            , borderColor (hex "3a96cf")
            , legacyBoxShadow "0 3px 15px 0 rgba(58, 150, 207, 0.1)"
            ]
        ]


algoliaDropdownMenuStyle : Model -> Style
algoliaDropdownMenuStyle model =
    Css.batch
        [ ifMobile model.width (width (vw 80)) (width (px 300))
        , border3 (px 2) solid (rgba 228 228 228 0.6)
        , borderTopWidth (px 1)
        , borderRadius (px 4)
        , fontSize (px 13)
        , legacyBoxShadow "4px 4px 0 rgba(241, 241, 241, 0.35)"
        , legacyBoxSizing "border-box"
        , position absolute
        , top (pct 100)
        , zIndex (int 100)
        , left (px 0)
        , right auto
        , display block
        ]


algoliaSuggestionStyle : Style
algoliaSuggestionStyle =
    Css.batch
        [ textDecoration none
        , padding (px 12)
        , borderTop3 (px 1) solid (rgba 228 228 228 0.6)
        , cursor pointer
        , legacyTransition ".2s"
        , legacyDisplayFlex
        , legacyJustifyContentSpaceBetween
        , legacyAlignItems "center"
        , hover [ backgroundColor (rgba 241 241 241 0.35) ]
        ]


onEnter : msg -> Attribute msg
onEnter onEnterAction =
    on "keydown" <|
        Json.andThen
            (\keyCode ->
                if keyCode == 13 then
                    Json.succeed onEnterAction

                else
                    Json.fail (String.fromInt keyCode)
            )
            keyCode


searchBox : Model -> Html Msg
searchBox model =
    let
        aisSearchBox : Html Msg
        aisSearchBox =
            input
                [ css [ algoliaSearchInputStyle model ]
                , type_ "search"
                , id "aa-search-input"
                , placeholder "Search packages"
                , autocomplete False
                , onEnter OnEnterPress
                , onInput (OnSearchInput (ifMobile model.width 3 5))
                , onBlur ClearPackages
                ]
                []

        aisDropdownMenu : Html Msg
        aisDropdownMenu =
            viewIf (not <| List.isEmpty model.packages) <|
                span
                    [ css [ algoliaDropdownMenuStyle model ] ]
                    [ div [] <| List.map (toDropdownMenuContent model) model.packages ]
    in
    div
        [ css
            [ children
                [ everything [ unselectable ]
                ]
            ]
        ]
        [ Html.Styled.form []
            [ span
                [ css
                    [ position relative
                    , display inlineBlock
                    ]
                ]
                [ aisSearchBox
                , aisDropdownMenu
                ]
            , label
                [ for "aa-search-input"
                , css
                    [ visibility hidden
                    , display block
                    ]
                ]
                [ text "Search packages" ]
            ]
        ]


toDropdownMenuContent : Model -> Package -> Html Msg
toDropdownMenuContent model package =
    a
        [ css [ algoliaSuggestionStyle ]
        , Route.href (Route.Packages Nothing)
        ]
        [ span
            [ css
                [ model.theme.color
                , Global.descendants
                    [ Global.em
                        [ fontWeight (int 700)
                        , fontStyle normal
                        , backgroundColor (rgba 58 150 207 0.1)
                        , padding4 (px 2) zero (px 2) (px 2)
                        ]
                    ]
                ]
            ]
          <|
            case Html.Parser.run package.nameHighlighted of
                Ok nameNode ->
                    Html.Parser.Util.toVirtualDom nameNode
                        |> List.map Html.Styled.fromUnstyled

                Err _ ->
                    []
        ]


fadeinStyle : Style
fadeinStyle =
    Css.batch
        [ opacity (int 0)
        , legacyTransform "translate(0, 50px)"
        , legacyTransition "all 300ms"
        ]


scrollinStyle : Style
scrollinStyle =
    Css.batch
        [ opacity (int 1)
        , legacyTransform "translate(0, 0)"
        ]


addFadeinStyle : Bool -> Style
addFadeinStyle bool =
    if bool then
        Css.batch [ fadeinStyle, scrollinStyle ]

    else
        fadeinStyle


getStartedStyle : Int -> Style
getStartedStyle currentWidth =
    Css.batch
        [ marginTop (px 80)
        , paddingTop (px 40)
        , ifMobile currentWidth (paddingLeft zero) (paddingLeft (px 50))
        , textAlign left
        , fontWeight (int 300)
        ]


getStartedView : Model -> Html Msg
getStartedView model =
    let
        pStyled : List (Attribute msg) -> List (Html msg) -> Html msg
        pStyled =
            styled p
                [ ifMobile model.width (width (vw 80)) (width (vw 35)) ]
    in
    div
        [ css
            [ getStartedStyle model.width
            , addFadeinStyle model.isFadein.getStart
            ]
        ]
        [ h1 [] [ text "Getting started" ]
        , pStyled
            []
            [ text """Want to use Poac right now?
                    Let's get started in just a sec!
                    Unless, if you use a peculiar an operating system,
                     you can install Poac easily by the following command.
                    """
            ]
        , pStyled
            [ css
                [ before
                    [ property "content" "$ "
                    ]
                , fontFamilies [ "Menlo", .value sansSerif ]
                , fontWeight (int 500)
                , padding (px 20)
                , marginBottom (px 30)
                , legacyBoxShadow "0 3px 15px 0 #b9b9b9"
                ]
            ]
            [ text "curl -fsSL https://sh.poac.pm | bash"
            ]
        , pStyled
            []
            [ text "Please refer to "
            , a [ href "https://docs.poac.io" ]
                [ text "The Poac Book" ]
            , text " for details."
            ]
        ]


cardStyle : Style
cardStyle =
    Css.batch
        [ padding (px 50)
        , verticalAlign top
        , textAlign center
        , hover
            [ legacyBoxShadow "0 3px 15px 0 #b9b9b9"
            ]
        ]


cardItemStyle : Style
cardItemStyle =
    Css.batch
        [ marginTop zero
        , fontStyle normal
        ]


sectionItem : Model -> String -> List String -> Html Msg
sectionItem model h2Text pTexts =
    div
        [ css
            [ cardStyle
            , addFadeinStyle model.isFadein.section1
            ]
        ]
    <|
        h2
            [ css
                [ cardItemStyle
                , marginBottom (px 30)
                ]
            ]
            [ text h2Text ]
            :: List.map
                (\t ->
                    p
                        [ css [ cardItemStyle, marginBottom zero ] ]
                        [ text t ]
                )
                pTexts


section : Model -> Html Msg
section model =
    div
        [ css
            [ width (vw 80)
            , paddingTop (px 80)
            , legacyDisplayFlex
            , legacyJustifyContentSpaceAround
            , ifMobile model.width (flexDirection column) (flexDirection row)
            ]
        ]
    <|
        List.map2 (sectionItem model)
            [ "Useful Interface", "Accelerate Development", "Open Source Software" ]
            [ [ """Poac is a C++ package manager and
                   a CLI application provided for a client."""
              , """Poac is easy to use because it refers to
                   the other package managers' modern interface."""
              ]
            , [ """Even if you have managed packages manually or
                   if you have used other package managers,
                   you can quickly introduce Poac."""
              , "Poac flexibly copes with small-scale and large-scale development."
              ]
            , [ "All related to Poac is open source."
              , """You can contribute to Poac by publishing packages,
                   or you can contribute to Poac directly."""
              , "The client-side is written in C++, and it will be self-hosted."
              ]
            ]
