module Views.Svgs exposing (..)

import ElmEscapeHtml exposing (unescape)
import Html
import Messages exposing (Msg)
import Svg exposing (..)
import Svg.Attributes exposing (..)
import List
import List.Extra


joinStr2 : String -> String -> Int -> String
joinStr2 str str2 int =
    if int == 0 then
        str2

    else
        joinStr2 str (str2 ++ str) (int - 1)


joinStr : String -> Int -> String
joinStr str int =
    if int == 0 then
        str

    else
        joinStr2 str str (int - 1)


getWidth : Int -> Int -> Attribute msg
getWidth widthSize size =
    if widthSize < 1150 then
        width "40"
    else
        width "70"


getViewBox : Int -> Int -> Attribute msg
getViewBox widthSize size =
    if widthSize < 1150 then
        viewBox "0 0 800 347"
    else
        viewBox "0 0 1060 460"


logo : Int -> Html.Html Msg
logo widthSize =
    svg
        [ getWidth widthSize 1150
        , height "40"
        , getViewBox widthSize 1150
        , version "1.1"
        , class "logo"
        ]
        [ defs []
            [ radialGradient
                [ cx "10.5172853%"
                , cy "100%"
                , fx "10.5172853%"
                , fy "100%"
                , r "119.945282%"
                , gradientTransform
                    """translate(0.105173,1.000000)
                          ,scale(1.000000,0.952381)
                          ,rotate(-51.739535)
                          ,scale(1.000000,0.892357)
                          ,translate(-0.105173,-1.000000)"""
                , id "radialGradient-1"
                ]
                [ stop
                    [ stopColor "#3023AE"
                    , offset "0%"
                    ]
                    []
                , stop
                    [ stopColor "#53A0FD"
                    , offset "79.8743881%"
                    ]
                    []
                , stop
                    [ stopColor "#51DEEC"
                    , offset "100%"
                    ]
                    []
                ]
            ]
        , g
            [ id "logo"
            , stroke "none"
            , strokeWidth "1"
            , fill "none"
            , fillRule "evenodd"
            ]
            [ g [ class "logo-g" ]
                [ g [ transform "translate(40.000000, 20.000000)" ]
                    [ Svg.path
                        [ d """M0,200 C0,0 183.916355,
                                 3.55271368e-15 200,0 C216.083645,0 400,0 400,200 C400,
                                 360.602127 263.263237,400 200,400 C169.940075,
                                 400 130.053436,383.872837 110.053436,360 C83.687159,
                                 328.528108 81.6540229,287.441333 100,280 C120,
                                 271.887777 144.702055,
                                 320 200,320 C255.297945,320 320,270.651856 320,200 C320,
                                 129.348144 280,80 200,80 C120,80 79.9607444,129.348144 79.9607444,
                                 200 L79.9607444,380 C79.9607444,400.484491 52.5173973,
                                 420 40,420 C27.4826027,420 7.10542736e-15,398.833096 0,
                                 380 L0,200 Z M200,260 C166.862915,
                                 260 140,233.137085 140,200 C140,166.862915 166.862915,140 200,
                                 140 C233.137085,140 260,166.862915 260,200 C260,
                                 233.137085 233.137085,260 200,260 Z"""
                        , class "icon"
                        , fill "url(#radialGradient-1)"
                        ]
                        []
                    , text_
                        [ class "poac"
                        , fontFamily "VarelaRound, Varela Round"
                        , fontSize "230"
                        , fontWeight "normal"
                        , letterSpacing "-12.5500002"
                        ]
                        [ tspan
                            [ x "423"
                            , y "367"
                            ]
                            [ text "poac" ]
                        ]
                    ]
                ]
            ]
        ]


spinner : Html.Html Msg
spinner =
    svg
        [ version "1.1"
        , id "loader-1"
        , x "0px"
        , y "0px"
        , width "40px"
        , height "40px"
        , viewBox "0 0 50 50"
        , Svg.Attributes.style "enable-background:new 0 0 50 50;"
        , xmlSpace "preserve"
        ]
        [ Svg.path
            [ fill "#000"
            , d """M43.935,25.145c0-10.318-8.364-18.683-18.683-18.683c-10.318,
                   0-18.683,8.365-18.683,18.683h4.068c0-8.071,6.543-14.615,
                   14.615-14.615c8.072,0,14.615,6.543,14.615,14.615H43.935z"""
            ]
            [ animateTransform
                [ attributeType "xml"
                , attributeName "transform"
                , type_ "rotate"
                , from "0 25 25"
                , to "360 25 25"
                , dur "0.6s"
                , repeatCount "indefinite"
                ]
                []
            ]
        ]


top : Int -> Html.Html Msg
top widthSize =
    Html.div [ class "top-svg-container" ]
        [ Html.div [ class "top-svg-row" ]
            [ content widthSize ]
        ]

content : Int -> Html.Html Msg
content widthSize =
    svg
        [ class "top-svg"
        , viewBox <|
            if widthSize < 1150 then
                "0 0 724 596"

            else
                "0 0 1100 796"

        --        , viewBox "0 0 1100 796"
        --        , viewBox "0 0 724 596"
        , version "1.1"
        ]
        [ defs []
            [ linearGradient
                [ x1 "50%"
                , y1 "0%"
                , x2 "50%"
                , y2 "100%"
                , id "linearGradient-1"
                ]
                [ stop
                    [ Svg.Attributes.stopColor "#FAD961"
                    , Svg.Attributes.offset "0%"
                    ]
                    []
                , stop
                    [ Svg.Attributes.stopColor "#F76B1C"
                    , Svg.Attributes.offset "100%"
                    ]
                    []
                ]
            ]
        , Svg.path
            [ d """M1048.06933,159.914465 C1035.81036,
                     92.4212078 957.22027,66.5430856 923.002604,
                     63.8577874 C883.887748,60.7881727 842.601285,
                     69.5212209 813.313645,83.0547212 C770.088876,
                     103.028417 740.037464,133.335355 646.172023,
                     136.53134 C548.90982,146.341703 342.277114,
                     56.7953732 276.865748,48.729089 C246.006514,
                     44.923644 231.041145,38.1520814 190.109334,
                     38.1520814 C146.120395,38.1520814 108.899748,
                     48.7039446 87.9310548,78.8051072 C55.7615357,
                     124.985378 72.2843138,180.121257 83.9275815,
                     202.548905 C95.2666361,224.390567 121.74435,
                     249.347569 121.74435,287.527061 C121.74435,
                     321.049304 112.493937,352.461688 82.1646443,
                     389.490524 C40.8168355,439.971795 2.99278216,
                     482.45382 8.842373,516.701646 C26.2369348,
                     618.542259 296.150867,758 548.90982,
                     758 C682.357164,758 840.109536,
                     716.65271 928.069071,671.127651 C1003.84926,
                     631.906233 1083.13166,590.917081 1079.99746,
                     542.00009 C1074.81487,461.112733 912.219418,
                     464.145084 912.219418,388.029995 C912.219418,
                     310.080874 1063.84966,246.795036 1048.06933,
                     159.914465 Z"""
            , id "background"
            , fill "url(#linearGradient-1)"
            ]
            []
        , terminalView
        , terminalAnimation widthSize
        ]


terminalView : Svg Msg
terminalView =
    g [ id "terminal" ]
        [ rect
            [ id "Rectangle"
            , fill "#555454"
            , x "128"
            , y "147"
            , width "708"
            , height "583"
            , rx "21"
            ]
            []
        , circle
            [ id "OvalRed"
            , fill "#FD5D57"
            , cx "169"
            , cy "183"
            , r "8"
            ]
            []
        , circle
            [ id "OvalYellow"
            , fill "#FEBD08"
            , cx "199"
            , cy "183"
            , r "8"
            ]
            []
        , circle
            [ id "OvalGreen"
            , fill "#15CD34"
            , cx "229"
            , cy "183"
            , r "8"
            ]
            []
        ]


-- no-break space
nbsp : String
nbsp =
    unescape "\u{00A0}"


textSize : Int
textSize = 8


animateElem : String -> String -> Svg msg
animateElem beginMs durMs =
    animate
        [ attributeName "display"
        , begin <| beginMs ++ "ms; anim_last.end" ++ (if beginMs == "0" then "" else "+" ++ beginMs ++ "ms")
        , dur <| durMs ++ "ms"
        , from "inline"
        , to "inline"
        ]
        []


instanceSvg : Int -> String -> Svg msg
instanceSvg scene command =
    let
        commandLength = textSize * (String.length command)
    in
    g [ id <| "g" ++ String.fromInt scene ]
      [ text_
          [ class "color5"
          , lengthAdjust "spacingAndGlyphs"
          , textLength "8"
          , x "0"
          ]
          [ text "❯" ]
      , text_
          [ class "foreground"
          , lengthAdjust "spacingAndGlyphs"
          , textLength <| String.fromInt (textSize + commandLength)
          , x "8"
          ]
          [ text <| nbsp ++ command ]
      , text_
          [ class "background"
          , lengthAdjust "spacingAndGlyphs"
          , textLength "8"
          , x <| String.fromInt (16 + commandLength)
          ]
          [ text nbsp ]
      ]


swapAppend : appendable -> appendable -> appendable
swapAppend =
    \b a -> a ++ b


createCommand : Int -> Int -> List String -> List (Svg msg)
createCommand lo hi commands =
    let
        scene_range =
            List.range lo hi
        scanned_command =
            commands
            |> List.Extra.scanl1 swapAppend
    in
        List.map2 instanceSvg scene_range scanned_command


newLine : Int -> String -> Svg msg
newLine scene class_name =
    g [ id <| "g" ++ String.fromInt scene ]
      [ text_
          [ class class_name
          , lengthAdjust "spacingAndGlyphs"
          , textLength "8"
          , x "0"
          ]
          [ text nbsp ]
      ]


commandLineDirectory : Int -> String -> Svg msg
commandLineDirectory scene dirname =
    g [ id <| "g" ++ String.fromInt scene ]
      [ text_
          [ class "color4"
          , lengthAdjust "spacingAndGlyphs"
          , textLength <| String.fromInt <| textSize * (String.length dirname)
          , x "0"
          ]
          [ text dirname ]
      ]


terminalAnimation : Int -> Html.Html Msg
terminalAnimation widthSize =
    svg
        [ id "screen"
        , viewBox <|
            if widthSize < 1150 then
                "-32 -53 800 510"

            else
                "-160 -200 800 510"
        , width "800"
        , fillOpacity "1.0"
        , preserveAspectRatio "xMidYMin meet"
        , version "1.1"
        ]
        [ defs [] <|
            [ newLine 1 "background" ] ++
            createCommand 2 18 [ "", "p", "o", "a", "c", nbsp, "n", "e", "w", nbsp, "m", "y", "_", "p", "r", "o", "j" ]
            ++
            [ newLine 19 "foreground"
            , commandLineDirectory 20 "/tmp"
            ] ++
            createCommand 21 25 [ "c", "d", nbsp, "m", "y" ]
            ++
            [ g [ id "g26" ]
                [ text_
                    [ class "color5"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "0"
                    ]
                    [ text "❯" ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "88"
                    , x "8"
                    ]
                    [ text <| nbsp ++ "cd" ++ nbsp ++ "my_proj" ]
                , text_
                    [ class "foreground"
                    , fontWeight "bold"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "96"
                    ]
                    [ text "/" ]
                , text_
                    [ class "background"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "104"
                    ]
                    [ text nbsp ]
                ]
            ] ++
            createCommand 27 27 [ "cd" ++ nbsp ++ "my_proj" ]
            ++
            createCommand 28 36 [ "t", "r", "e", "e", nbsp, ".", nbsp, "-", "a" ]
            ++
            [ commandLineDirectory 37 "/tmp/my_proj" ] ++
            createCommand 38 40 [ "poac" ++ nbsp ++ "r", "u", "n" ]
            ++
            [ g [ id "g41" ]
                [ text_
                    [ class "color5"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "0"
                    ]
                    [ text "❯" ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "136"
                    , x "8"
                    ]
                    [ text <| nbsp ++ "poac" ++ nbsp ++ "new" ++ nbsp ++ "my_proj" ]
                ]
            , g [ id "g42" ]
                [ text_
                    [ class "foreground"
                    , fontWeight "bold"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "384"
                    , x "0"
                    ]
                    [ text <| "Your" ++ nbsp ++ "\"my_proj\"" ++ nbsp ++ "project" ++ nbsp ++ "was" ++ nbsp ++ "created" ++ nbsp ++ "successfully." ]
                ]
            , g [ id "g43" ]
                [ text_
                    [ class "foreground"
                    , fontWeight "bold"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "256"
                    , x "0"
                    ]
                    [ text <| "Go" ++ nbsp ++ "into" ++ nbsp ++ "your" ++ nbsp ++ "project" ++ nbsp ++ "by" ++ nbsp ++ "running:" ]
                ]
            , g [ id "g44" ]
                [ text_
                    [ class "foreground"
                    , fontWeight "bold"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "128"
                    , x "0"
                    ]
                    [ text <| joinStr nbsp 5 ++ "cd my_proj" ]
                ]
            , g [ id "g45" ]
                [ text_
                    [ class "foreground"
                    , fontWeight "bold"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "192"
                    , x "0"
                    ]
                    [ text <| "Start" ++ nbsp ++ "your" ++ nbsp ++ "project" ++ nbsp ++ "with:" ]
                ]
            , g [ id "g46" ]
                [ text_
                    [ class "foreground"
                    , fontWeight "bold"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "112"
                    , x "0"
                    ]
                    [ text <| joinStr nbsp 5 ++ "poac run" ]
                ]
            , g [ id "g47" ]
                [ text_
                    [ class "color5"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "0"
                    ]
                    [ text "❯" ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "88"
                    , x "8"
                    ]
                    [ text <| nbsp ++ "cd" ++ nbsp ++ "my_proj" ]
                ]
            , g [ id "g48" ]
                [ text_
                    [ class "color5"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "0"
                    ]
                    [ text "❯" ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "80"
                    , x "8"
                    ]
                    [ text <| nbsp ++ "tree" ++ nbsp ++ "." ++ nbsp ++ "-a" ]
                ]
            , g [ id "g52" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "96"
                    , x "0"
                    ]
                    [ text "├── main.cpp" ]
                ]
            , g [ id "g54" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "176"
                    , x "0"
                    ]
                    [ text "0 directories, 4 files" ]
                ]
            , g [ id "g53" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "96"
                    , x "0"
                    ]
                    [ text "└── poac.yml" ]
                ]
            , g [ id "g50" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "112"
                    , x "0"
                    ]
                    [ text "├── .gitignore" ]
                ]
            , commandLineDirectory 49 "."
            , g [ id "g51" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "104"
                    , x "0"
                    ]
                    [ text "├── README.md" ]
                ]
            , g [ id "g55" ]
                [ text_
                    [ class "color5"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "0"
                    ]
                    [ text "❯" ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "72"
                    , x "8"
                    ]
                    [ text <| nbsp ++ "poac run" ]
                ]
            , g [ id "g56" ]
                [ text_
                    [ class "color2"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "80"
                    , x "0"
                    ]
                    [ text <| "Compiled:" ++ nbsp ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "240"
                    , x "80"
                    ]
                    [ text "Output to `_build/bin/my_proj`" ]
                ]
            , g [ id "g57" ]
                [ text_
                    [ class "color2"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "72"
                    , x "0"
                    ]
                    [ text <| "Running:" ++ nbsp ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "160"
                    , x "72"
                    ]
                    [ text "`_build/bin/my_proj`" ]
                ]
            , g [ id "g58" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "104"
                    , x "0"
                    ]
                    [ text "Hello, world!" ]
                ]
            , g [ id "g59" ]
                [ text_
                    [ class "color5"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "0"
                    ]
                    [ text "❯"
                    ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "504"
                    , x "8"
                    ]
                    [ text <| nbsp ++ "echo \"deps:\\n  boost/bind: \\\">=1.64.0 and <1.68.0\\\"\" >> poac.yml" ]
                , text_
                    [ class "background"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "496"
                    ]
                    [ text nbsp ]
                ]
            , g [ id "g60" ]
                [ text_
                    [ class "color5"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "0"
                    ]
                    [ text "❯" ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "504"
                    , x "8"
                    ]
                    [ text <| nbsp ++ "echo \"deps:\\n  boost/bind: \\\">=1.64.0 and <1.68.0\\\"\" >> poac.yml" ]
                ]
            ] ++
            createCommand 61 67 [ "poac" ++ nbsp ++ "i", "n", "s", "t", "a", "l", "l" ]
            ++
            [ g [ id "g68" ]
                [ text_
                    [ class "color5"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "0"
                    ]
                    [ text "❯" ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "104"
                    , x "8"
                    ]
                    [ text <| nbsp ++ "poac" ++ nbsp ++ "install" ]
                ]
            , g [ id "g69" ]
                [ text_
                    [ class "color2"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "32"
                    , x "0"
                    ]
                    [ text <| "==>" ++ nbsp ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "168"
                    , x "32"
                    ]
                    [ text <| "Resolving" ++ nbsp ++ "packages..." ]
                ]
            , g [ id "g70" ]
                [ text_
                    [ class "color2"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "32"
                    , x "0"
                    ]
                    [ text <| "==>" ++ nbsp ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "200"
                    , x "32"
                    ]
                    [ text "Resolving dependencies..." ]
                ]
            , g [ id "g71" ]
                [ text_
                    [ class "color2"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "32"
                    , x "0"
                    ]
                    [ text <| "==>" ++ nbsp ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "88"
                    , x "32"
                    ]
                    [ text "Fetching..." ]
                ]
            , g [ id "g72" ]
                [ text_
                    [ class "color2"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "40"
                    , x "0"
                    ]
                    [ text <| nbsp ++ nbsp ++ "●" ++ nbsp ++ nbsp ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "240"
                    , x "40"
                    ]
                    [ text "boost/bind 1.66.0 (from: poac)" ]
                ]
            , g [ id "g73" ]
                [ text_
                    [ class "color2"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "40"
                    , x "0"
                    ]
                    [ text <| nbsp ++ nbsp ++ "●" ++ nbsp ++ nbsp ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "256"
                    , x "40"
                    ]
                    [ text "boost/assert 1.66.0 (from: poac)" ]
                ]
            , g [ id "g74" ]
                [ text_
                    [ class "color2"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "40"
                    , x "0"
                    ]
                    [ text <| nbsp ++ nbsp ++ "●" ++ nbsp ++ nbsp ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "256"
                    , x "40"
                    ]
                    [ text "boost/config 1.66.0 (from: poac)" ]
                ]
            , g [ id "g75" ]
                [ text_
                    [ class "color2"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "40"
                    , x "0"
                    ]
                    [ text <| nbsp ++ nbsp ++ "●" ++ nbsp ++ nbsp ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "240"
                    , x "40"
                    ]
                    [ text "boost/core 1.66.0 (from: poac)" ]
                ]
            , g [ id "g76" ]
                [ text_
                    [ class "color2"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "32"
                    , x "0"
                    ]
                    [ text <| "==>" ++ nbsp ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "40"
                    , x "32"
                    ]
                    [ text "Done." ]
                ]
            ] ++
            createCommand 77 85 [ "tree d", "e", "p", "s", nbsp, "-", "L", nbsp, "1" ]
            ++
            [ g [ id "g86" ]
                [ text_
                    [ class "color4"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "96"
                    , x "0"
                    ]
                    [ text "/tmp/my_proj" ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "96"
                    ]
                    [ text nbsp ]
                , text_
                    [ class "color3"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "16"
                    , x "104"
                    ]
                    [ text "7s" ]
                ]
            , g [ id "g87" ]
                [ text_
                    [ class "color5"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "0"
                    ]
                    [ text "❯" ]
                , text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "120"
                    , x "8"
                    ]
                    [ text <| nbsp ++ "tree deps" ++ nbsp ++ "-L 1" ]
                ]
            , g [ id "g92" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "32"
                    , x "0"
                    ]
                    [ text <| "└──" ++ nbsp ]
                , text_
                    [ class "color4"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "80"
                    , x "32"
                    ]
                    [ text "boost-core" ]
                ]
            , g [ id "g93" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "176"
                    , x "0"
                    ]
                    [ text "4 directories, 0 files" ]
                ]
            , g [ id "g91" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "32"
                    , x "0"
                    ]
                    [ text <| "├──" ++ nbsp ]
                , text_
                    [ class "color4"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "96"
                    , x "32"
                    ]
                    [ text "boost-config" ]
                ]
            , commandLineDirectory 88 "deps"
            , g [ id "g89" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "32"
                    , x "0"
                    ]
                    [ text <| "├──" ++ nbsp ]
                , text_
                    [ class "color4"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "96"
                    , x "32"
                    ]
                    [ text "boost-assert" ]
                ]
            , g [ id "g90" ]
                [ text_
                    [ class "foreground"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "32"
                    , x "0"
                    ]
                    [ text <| "├──" ++ nbsp ]
                , text_
                    [ class "color4"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "80"
                    , x "32"
                    ]
                    [ text "boost-bind" ]
                ]
            , g [ id "g94" ]
                [ text_
                    [ class "color5"
                    , lengthAdjust "spacingAndGlyphs"
                    , textLength "8"
                    , x "0"
                    ]
                    [ text "❯" ]
                ]
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "0" ] []
            , use [ y "0", xlinkHref "#g1" ] []
            , animateElem "0" "3"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "34" ] []
            , use [ y "34", xlinkHref "#g2" ] []
            , animateElem "3" "1412"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "24", y "34" ] []
            , use [ y "34", xlinkHref "#g3" ] []
            , animateElem "1415" "50"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "32", y "34" ] []
            , use [ y "34", xlinkHref "#g4" ] []
            , animateElem "1465" "69"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "40", y "34" ] []
            , use [ y "34", xlinkHref "#g5" ] []
            , animateElem "1534" "79"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "48", y "34" ] []
            , use [ y "34", xlinkHref "#g6" ] []
            , animateElem "1613" "306"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "56", y "34" ] []
            , use [ y "34", xlinkHref "#g7" ] []
            , animateElem "1919" "228"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "64", y "34" ] []
            , use [ y "34", xlinkHref "#g8" ] []
            , animateElem "2147" "81"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "72", y "34" ] []
            , use [ y "34", xlinkHref "#g9" ] []
            , animateElem "2228" "73"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "80", y "34" ] []
            , use [ y "34", xlinkHref "#g10" ] []
            , animateElem "2301" "249"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "88", y "34" ] []
            , use [ y "34", xlinkHref "#g11" ] []
            , animateElem "2550" "137"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "96", y "34" ] []
            , use [ y "34", xlinkHref "#g12" ] []
            , animateElem "2687" "255"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "104", y "34" ] []
            , use [ y "34", xlinkHref "#g13" ] []
            , animateElem "2942" "275"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "112", y "34" ] []
            , use [ y "34", xlinkHref "#g14" ] []
            , animateElem "3217" "302"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "120", y "34" ] []
            , use [ y "34", xlinkHref "#g15" ] []
            , animateElem "3519" "249"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "128", y "34" ] []
            , use [ y "34", xlinkHref "#g16" ] []
            , animateElem "3768" "103"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "136", y "34" ] []
            , use [ y "34", xlinkHref "#g17" ] []
            , animateElem "3871" "269"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "144", y "34" ] []
            , use [ y "34", xlinkHref "#g18" ] []
            , animateElem "4140" "477"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g19" ] []
            , use [ y "17", xlinkHref "#g20" ] []
            , animateElem "3" "4615"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "144", y "34" ] []
            , use [ y "34", xlinkHref "#g18" ] []
            , animateElem "4617" "1"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "51" ] []
            , use [ y "51", xlinkHref "#g1" ] []
            , animateElem "4618" "9"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "51" ] []
            , use [ y "51", xlinkHref "#g1" ] []
            , animateElem "4627" "3"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "51" ] []
            , use [ y "51", xlinkHref "#g1" ] []
            , animateElem "4630" "18"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "221" ] []
            , use [ y "221", xlinkHref "#g1" ] []
            , animateElem "4648" "2"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "221" ] []
            , use [ y "221", xlinkHref "#g1" ] []
            , animateElem "4650" "3"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "255" ] []
            , use [ y "255", xlinkHref "#g2" ] []
            , animateElem "4653" "1"
            ]
        , g [ display "none" ]
            [ use [ y "221", xlinkHref "#g19" ] []
            , use [ y "238", xlinkHref "#g20" ] []
            , animateElem "4653" "4"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "255" ] []
            , use [ y "255", xlinkHref "#g2" ] []
            , animateElem "4654" "3"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "255" ] []
            , use [ y "255", xlinkHref "#g2" ] []
            , animateElem "4657" "518"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "24", y "255" ] []
            , use [ y "255", xlinkHref "#g21" ] []
            , animateElem "5175" "57"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "32", y "255" ] []
            , use [ y "255", xlinkHref "#g22" ] []
            , animateElem "5232" "205"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "40", y "255" ] []
            , use [ y "255", xlinkHref "#g23" ] []
            , animateElem "5437" "127"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "48", y "255" ] []
            , use [ y "255", xlinkHref "#g24" ] []
            , animateElem "5564" "350"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "56", y "255" ] []
            , use [ y "255", xlinkHref "#g25" ] []
            , animateElem "5914" "162"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "104", y "255" ] []
            , use [ y "255", xlinkHref "#g26" ] []
            , animateElem "6076" "513"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "96", y "255" ] []
            , use [ y "255", xlinkHref "#g27" ] []
            , animateElem "6589" "2"
            ]
        , g [ display "none" ]
            [ use [ y "221", xlinkHref "#g19" ] []
            , use [ y "238", xlinkHref "#g20" ] []
            , animateElem "4657" "1935"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "96", y "255" ] []
            , use [ y "255", xlinkHref "#g27" ] []
            , animateElem "6591" "1"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "272" ] []
            , use [ y "272", xlinkHref "#g1" ] []
            , animateElem "6592" "1"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "272" ] []
            , use [ y "272", xlinkHref "#g1" ] []
            , animateElem "6593" "2"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "306" ] []
            , use [ y "306", xlinkHref "#g2" ] []
            , animateElem "6595" "1113"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "24", y "306" ] []
            , use [ y "306", xlinkHref "#g28" ] []
            , animateElem "7708" "253"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "32", y "306" ] []
            , use [ y "306", xlinkHref "#g29" ] []
            , animateElem "7961" "365"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "40", y "306" ] []
            , use [ y "306", xlinkHref "#g30" ] []
            , animateElem "8326" "134"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "48", y "306" ] []
            , use [ y "306", xlinkHref "#g31" ] []
            , animateElem "8460" "330"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "56", y "306" ] []
            , use [ y "306", xlinkHref "#g32" ] []
            , animateElem "8790" "571"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "64", y "306" ] []
            , use [ y "306", xlinkHref "#g33" ] []
            , animateElem "9361" "379"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "72", y "306" ] []
            , use [ y "306", xlinkHref "#g34" ] []
            , animateElem "9740" "225"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "80", y "306" ] []
            , use [ y "306", xlinkHref "#g35" ] []
            , animateElem "9965" "259"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "88", y "306" ] []
            , use [ y "306", xlinkHref "#g36" ] []
            , animateElem "10224" "212"
            ]
        , g [ display "none" ]
            [ use [ y "272", xlinkHref "#g19" ] []
            , use [ y "289", xlinkHref "#g37" ] []
            , animateElem "6595" "3842"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "88", y "306" ] []
            , use [ y "306", xlinkHref "#g36" ] []
            , animateElem "10436" "1"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "323" ] []
            , use [ y "323", xlinkHref "#g1" ] []
            , animateElem "10437" "5"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "442" ] []
            , use [ y "442", xlinkHref "#g1" ] []
            , animateElem "10442" "2"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "442" ] []
            , use [ y "442", xlinkHref "#g1" ] []
            , animateElem "10444" "1"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "476" ] []
            , use [ y "476", xlinkHref "#g2" ] []
            , animateElem "10445" "1611"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "24", y "476" ] []
            , use [ y "476", xlinkHref "#g3" ] []
            , animateElem "12056" "49"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "32", y "476" ] []
            , use [ y "476", xlinkHref "#g4" ] []
            , animateElem "12105" "105"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "40", y "476" ] []
            , use [ y "476", xlinkHref "#g5" ] []
            , animateElem "12210" "74"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "48", y "476" ] []
            , use [ y "476", xlinkHref "#g6" ] []
            , animateElem "12284" "301"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "56", y "476" ] []
            , use [ y "476", xlinkHref "#g7" ] []
            , animateElem "12585" "208"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "64", y "476" ] []
            , use [ y "476", xlinkHref "#g38" ] []
            , animateElem "12793" "129"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "72", y "476" ] []
            , use [ y "476", xlinkHref "#g39" ] []
            , animateElem "12922" "105"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "80", y "476" ] []
            , use [ y "476", xlinkHref "#g40" ] []
            , animateElem "13027" "821"
            ]
        , g [ display "none" ]
            [ use [ y "442", xlinkHref "#g19" ] []
            , use [ y "459", xlinkHref "#g37" ] []
            , animateElem "10445" "3404"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "80", y "476" ] []
            , use [ y "476", xlinkHref "#g40" ] []
            , animateElem "13848" "1"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "13849" "1"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g19" ] []
            , use [ y "17", xlinkHref "#g20" ] []
            , use [ y "34", xlinkHref "#g41" ] []
            , animateElem "4618" "9972"
            ]
        , g [ display "none" ]
            [ use [ y "68", xlinkHref "#g42" ] []
            , use [ y "119", xlinkHref "#g43" ] []
            , use [ y "136", xlinkHref "#g44" ] []
            , use [ y "170", xlinkHref "#g45" ] []
            , use [ y "187", xlinkHref "#g46" ] []
            , animateElem "4648" "9942"
            ]
        , g [ display "none" ]
            [ use [ y "221", xlinkHref "#g19" ] []
            , use [ y "238", xlinkHref "#g20" ] []
            , use [ y "255", xlinkHref "#g47" ] []
            , animateElem "6592" "7998"
            ]
        , g [ display "none" ]
            [ use [ y "272", xlinkHref "#g19" ] []
            , use [ y "289", xlinkHref "#g37" ] []
            , use [ y "306", xlinkHref "#g48" ] []
            , animateElem "10437" "4153"
            ]
        , g [ display "none" ]
            [ use [ y "323", xlinkHref "#g49" ] []
            , use [ y "340", xlinkHref "#g50" ] []
            , use [ y "357", xlinkHref "#g51" ] []
            , use [ y "374", xlinkHref "#g52" ] []
            , use [ y "391", xlinkHref "#g53" ] []
            , use [ y "425", xlinkHref "#g54" ] []
            , animateElem "10442" "4148"
            ]
        , g [ display "none" ]
            [ use [ y "442", xlinkHref "#g19" ] []
            , use [ y "459", xlinkHref "#g37" ] []
            , use [ y "476", xlinkHref "#g55" ] []
            , animateElem "13849" "741"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "13850" "740"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g41" ] []
            , use [ y "34", xlinkHref "#g42" ] []
            , use [ y "85", xlinkHref "#g43" ] []
            , use [ y "102", xlinkHref "#g44" ] []
            , use [ y "136", xlinkHref "#g45" ] []
            , use [ y "153", xlinkHref "#g46" ] []
            , use [ y "187", xlinkHref "#g19" ] []
            , use [ y "204", xlinkHref "#g20" ] []
            , use [ y "221", xlinkHref "#g47" ] []
            , use [ y "238", xlinkHref "#g19" ] []
            , use [ y "255", xlinkHref "#g37" ] []
            , use [ y "272", xlinkHref "#g48" ] []
            , use [ y "289", xlinkHref "#g49" ] []
            , use [ y "306", xlinkHref "#g50" ] []
            , use [ y "323", xlinkHref "#g51" ] []
            , use [ y "340", xlinkHref "#g52" ] []
            , use [ y "357", xlinkHref "#g53" ] []
            , use [ y "391", xlinkHref "#g54" ] []
            , use [ y "408", xlinkHref "#g19" ] []
            , use [ y "425", xlinkHref "#g37" ] []
            , use [ y "442", xlinkHref "#g55" ] []
            , use [ y "459", xlinkHref "#g56" ] []
            , use [ y "476", xlinkHref "#g57" ] []
            , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "14590" "4"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "14594" "5"
            ]
        , g [ display "none" ]
            [ use [ y "17", xlinkHref "#g42" ] []
            , use [ y "68", xlinkHref "#g43" ] []
            , use [ y "85", xlinkHref "#g44" ] []
            , use [ y "119", xlinkHref "#g45" ] []
            , use [ y "136", xlinkHref "#g46" ] []
            , use [ y "170", xlinkHref "#g19" ] []
            , use [ y "187", xlinkHref "#g20" ] []
            , use [ y "204", xlinkHref "#g47" ] []
            , use [ y "221", xlinkHref "#g19" ] []
            , use [ y "238", xlinkHref "#g37" ] []
            , use [ y "255", xlinkHref "#g48" ] []
            , use [ y "272", xlinkHref "#g49" ] []
            , use [ y "289", xlinkHref "#g50" ] []
            , use [ y "306", xlinkHref "#g51" ] []
            , use [ y "323", xlinkHref "#g52" ] []
            , use [ y "340", xlinkHref "#g53" ] []
            , use [ y "374", xlinkHref "#g54" ] []
            , use [ y "391", xlinkHref "#g19" ] []
            , use [ y "408", xlinkHref "#g37" ] []
            , use [ y "425", xlinkHref "#g55" ] []
            , use [ y "442", xlinkHref "#g56" ] []
            , use [ y "459", xlinkHref "#g57" ] []
            , use [ y "476", xlinkHref "#g58" ] []
            , animateElem "14594" "10"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "14599" "5"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
            , use [ y "493", xlinkHref "#g2" ] []
            , animateElem "14604" "4511"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "384", y "493" ] []
            , use [ y "493", xlinkHref "#g59" ] []
            , animateElem "19115" "298"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "384", y "493" ] []
            , use [ y "493", xlinkHref "#g59" ] []
            , animateElem "19413" "2"
            ]
        , g [ display "none" ]
            [ use [ y "34", xlinkHref "#g43" ] []
            , use [ y "51", xlinkHref "#g44" ] []
            , use [ y "85", xlinkHref "#g45" ] []
            , use [ y "102", xlinkHref "#g46" ] []
            , use [ y "136", xlinkHref "#g19" ] []
            , use [ y "153", xlinkHref "#g20" ] []
            , use [ y "170", xlinkHref "#g47" ] []
            , use [ y "187", xlinkHref "#g19" ] []
            , use [ y "204", xlinkHref "#g37" ] []
            , use [ y "221", xlinkHref "#g48" ] []
            , use [ y "238", xlinkHref "#g49" ] []
            , use [ y "255", xlinkHref "#g50" ] []
            , use [ y "272", xlinkHref "#g51" ] []
            , use [ y "289", xlinkHref "#g52" ] []
            , use [ y "306", xlinkHref "#g53" ] []
            , use [ y "340", xlinkHref "#g54" ] []
            , use [ y "357", xlinkHref "#g19" ] []
            , use [ y "374", xlinkHref "#g37" ] []
            , use [ y "391", xlinkHref "#g55" ] []
            , use [ y "408", xlinkHref "#g56" ] []
            , use [ y "425", xlinkHref "#g57" ] []
            , use [ y "442", xlinkHref "#g58" ] []
            , use [ y "459", xlinkHref "#g19" ] []
            , use [ y "476", xlinkHref "#g37" ] []
            , animateElem "14604" "4813"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "384", y "493" ] []
            , use [ y "493", xlinkHref "#g59" ] []
            , animateElem "19415" "2"
            ]
        , g [ display "none" ]
            [ use [ y "17", xlinkHref "#g43" ] []
            , use [ y "34", xlinkHref "#g44" ] []
            , use [ y "68", xlinkHref "#g45" ] []
            , use [ y "85", xlinkHref "#g46" ] []
            , use [ y "119", xlinkHref "#g19" ] []
            , use [ y "136", xlinkHref "#g20" ] []
            , use [ y "153", xlinkHref "#g47" ] []
            , use [ y "170", xlinkHref "#g19" ] []
            , use [ y "187", xlinkHref "#g37" ] []
            , use [ y "204", xlinkHref "#g48" ] []
            , use [ y "221", xlinkHref "#g49" ] []
            , use [ y "238", xlinkHref "#g50" ] []
            , use [ y "255", xlinkHref "#g51" ] []
            , use [ y "272", xlinkHref "#g52" ] []
            , use [ y "289", xlinkHref "#g53" ] []
            , use [ y "323", xlinkHref "#g54" ] []
            , use [ y "340", xlinkHref "#g19" ] []
            , use [ y "357", xlinkHref "#g37" ] []
            , use [ y "374", xlinkHref "#g55" ] []
            , use [ y "391", xlinkHref "#g56" ] []
            , use [ y "408", xlinkHref "#g57" ] []
            , use [ y "425", xlinkHref "#g58" ] []
            , use [ y "442", xlinkHref "#g19" ] []
            , use [ y "459", xlinkHref "#g37" ] []
            , use [ y "476", xlinkHref "#g60" ] []
            , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "19417" "1"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
            , use [ y "493", xlinkHref "#g2" ] []
            , animateElem "19418" "10"
            ]
        , g [ display "none" ]
            [ use [ y "459", xlinkHref "#g19" ] []
            , use [ y "476", xlinkHref "#g37" ] []
            , animateElem "19418" "17"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
            , use [ y "493", xlinkHref "#g2" ] []
            , animateElem "19428" "7"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
            , use [ y "493", xlinkHref "#g2" ] []
            , animateElem "19435" "2250"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "24", y "493" ] []
            , use [ y "493", xlinkHref "#g3" ] []
            , animateElem "21685" "67"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "32", y "493" ] []
            , use [ y "493", xlinkHref "#g4" ] []
            , animateElem "21752" "41"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "40", y "493" ] []
            , use [ y "493", xlinkHref "#g5" ] []
            , animateElem "21793" "42"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "48", y "493" ] []
            , use [ y "493", xlinkHref "#g6" ] []
            , animateElem "21835" "181"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "56", y "493" ] []
            , use [ y "493", xlinkHref "#g7" ] []
            , animateElem "22016" "133"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "64", y "493" ] []
            , use [ y "493", xlinkHref "#g61" ] []
            , animateElem "22149" "79"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "72", y "493" ] []
            , use [ y "493", xlinkHref "#g62" ] []
            , animateElem "22228" "96"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "80", y "493" ] []
            , use [ y "493", xlinkHref "#g63" ] []
            , animateElem "22324" "148"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "88", y "493" ] []
            , use [ y "493", xlinkHref "#g64" ] []
            , animateElem "22472" "127"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "96", y "493" ] []
            , use [ y "493", xlinkHref "#g65" ] []
            , animateElem "22599" "160"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "104", y "493" ] []
            , use [ y "493", xlinkHref "#g66" ] []
            , animateElem "22759" "102"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "112", y "493" ] []
            , use [ y "493", xlinkHref "#g67" ] []
            , animateElem "22861" "293"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g44" ] []
            , use [ y "34", xlinkHref "#g45" ] []
            , use [ y "51", xlinkHref "#g46" ] []
            , use [ y "85", xlinkHref "#g19" ] []
            , use [ y "102", xlinkHref "#g20" ] []
            , use [ y "119", xlinkHref "#g47" ] []
            , use [ y "136", xlinkHref "#g19" ] []
            , use [ y "153", xlinkHref "#g37" ] []
            , use [ y "170", xlinkHref "#g48" ] []
            , use [ y "187", xlinkHref "#g49" ] []
            , use [ y "204", xlinkHref "#g50" ] []
            , use [ y "221", xlinkHref "#g51" ] []
            , use [ y "238", xlinkHref "#g52" ] []
            , use [ y "255", xlinkHref "#g53" ] []
            , use [ y "289", xlinkHref "#g54" ] []
            , use [ y "306", xlinkHref "#g19" ] []
            , use [ y "323", xlinkHref "#g37" ] []
            , use [ y "340", xlinkHref "#g55" ] []
            , use [ y "357", xlinkHref "#g56" ] []
            , use [ y "374", xlinkHref "#g57" ] []
            , use [ y "391", xlinkHref "#g58" ] []
            , use [ y "408", xlinkHref "#g19" ] []
            , use [ y "425", xlinkHref "#g37" ] []
            , use [ y "442", xlinkHref "#g60" ] []
            , animateElem "19418" "3737"
            ]
        , g [ display "none" ]
            [ use [ y "459", xlinkHref "#g19" ] []
            , use [ y "476", xlinkHref "#g37" ] []
            , animateElem "19435" "3720"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "112", y "493" ] []
            , use [ y "493", xlinkHref "#g67" ] []
            , animateElem "23154" "1"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "23155" "1"
            ]
        , g [ display "none" ]
            [ use [ y "17", xlinkHref "#g45" ] []
            , use [ y "34", xlinkHref "#g46" ] []
            , use [ y "68", xlinkHref "#g19" ] []
            , use [ y "85", xlinkHref "#g20" ] []
            , use [ y "102", xlinkHref "#g47" ] []
            , use [ y "119", xlinkHref "#g19" ] []
            , use [ y "136", xlinkHref "#g37" ] []
            , use [ y "153", xlinkHref "#g48" ] []
            , use [ y "170", xlinkHref "#g49" ] []
            , use [ y "187", xlinkHref "#g50" ] []
            , use [ y "204", xlinkHref "#g51" ] []
            , use [ y "221", xlinkHref "#g52" ] []
            , use [ y "238", xlinkHref "#g53" ] []
            , use [ y "272", xlinkHref "#g54" ] []
            , use [ y "289", xlinkHref "#g19" ] []
            , use [ y "306", xlinkHref "#g37" ] []
            , use [ y "323", xlinkHref "#g55" ] []
            , use [ y "340", xlinkHref "#g56" ] []
            , use [ y "357", xlinkHref "#g57" ] []
            , use [ y "374", xlinkHref "#g58" ] []
            , use [ y "391", xlinkHref "#g19" ] []
            , use [ y "408", xlinkHref "#g37" ] []
            , use [ y "425", xlinkHref "#g60" ] []
            , use [ y "442", xlinkHref "#g19" ] []
            , use [ y "459", xlinkHref "#g37" ] []
            , use [ y "476", xlinkHref "#g68" ] []
            , animateElem "23155" "18"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "23156" "17"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g45" ] []
            , use [ y "17", xlinkHref "#g46" ] []
            , use [ y "51", xlinkHref "#g19" ] []
            , use [ y "68", xlinkHref "#g20" ] []
            , use [ y "85", xlinkHref "#g47" ] []
            , use [ y "102", xlinkHref "#g19" ] []
            , use [ y "119", xlinkHref "#g37" ] []
            , use [ y "136", xlinkHref "#g48" ] []
            , use [ y "153", xlinkHref "#g49" ] []
            , use [ y "170", xlinkHref "#g50" ] []
            , use [ y "187", xlinkHref "#g51" ] []
            , use [ y "204", xlinkHref "#g52" ] []
            , use [ y "221", xlinkHref "#g53" ] []
            , use [ y "255", xlinkHref "#g54" ] []
            , use [ y "272", xlinkHref "#g19" ] []
            , use [ y "289", xlinkHref "#g37" ] []
            , use [ y "306", xlinkHref "#g55" ] []
            , use [ y "323", xlinkHref "#g56" ] []
            , use [ y "340", xlinkHref "#g57" ] []
            , use [ y "357", xlinkHref "#g58" ] []
            , use [ y "374", xlinkHref "#g19" ] []
            , use [ y "391", xlinkHref "#g37" ] []
            , use [ y "408", xlinkHref "#g60" ] []
            , use [ y "425", xlinkHref "#g19" ] []
            , use [ y "442", xlinkHref "#g37" ] []
            , use [ y "459", xlinkHref "#g68" ] []
            , use [ y "476", xlinkHref "#g69" ] []
            , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "23173" "528"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g46" ] []
            , use [ y "34", xlinkHref "#g19" ] []
            , use [ y "51", xlinkHref "#g20" ] []
            , use [ y "68", xlinkHref "#g47" ] []
            , use [ y "85", xlinkHref "#g19" ] []
            , use [ y "102", xlinkHref "#g37" ] []
            , use [ y "119", xlinkHref "#g48" ] []
            , use [ y "136", xlinkHref "#g49" ] []
            , use [ y "153", xlinkHref "#g50" ] []
            , use [ y "170", xlinkHref "#g51" ] []
            , use [ y "187", xlinkHref "#g52" ] []
            , use [ y "204", xlinkHref "#g53" ] []
            , use [ y "238", xlinkHref "#g54" ] []
            , use [ y "255", xlinkHref "#g19" ] []
            , use [ y "272", xlinkHref "#g37" ] []
            , use [ y "289", xlinkHref "#g55" ] []
            , use [ y "306", xlinkHref "#g56" ] []
            , use [ y "323", xlinkHref "#g57" ] []
            , use [ y "340", xlinkHref "#g58" ] []
            , use [ y "357", xlinkHref "#g19" ] []
            , use [ y "374", xlinkHref "#g37" ] []
            , use [ y "391", xlinkHref "#g60" ] []
            , use [ y "408", xlinkHref "#g19" ] []
            , use [ y "425", xlinkHref "#g37" ] []
            , use [ y "442", xlinkHref "#g68" ] []
            , use [ y "459", xlinkHref "#g69" ] []
            , use [ y "476", xlinkHref "#g70" ] []
            , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "23701" "6049"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g19" ] []
            , use [ y "17", xlinkHref "#g20" ] []
            , use [ y "34", xlinkHref "#g47" ] []
            , use [ y "51", xlinkHref "#g19" ] []
            , use [ y "68", xlinkHref "#g37" ] []
            , use [ y "85", xlinkHref "#g48" ] []
            , use [ y "102", xlinkHref "#g49" ] []
            , use [ y "119", xlinkHref "#g50" ] []
            , use [ y "136", xlinkHref "#g51" ] []
            , use [ y "153", xlinkHref "#g52" ] []
            , use [ y "170", xlinkHref "#g53" ] []
            , use [ y "204", xlinkHref "#g54" ] []
            , use [ y "221", xlinkHref "#g19" ] []
            , use [ y "238", xlinkHref "#g37" ] []
            , use [ y "255", xlinkHref "#g55" ] []
            , use [ y "272", xlinkHref "#g56" ] []
            , use [ y "289", xlinkHref "#g57" ] []
            , use [ y "306", xlinkHref "#g58" ] []
            , use [ y "323", xlinkHref "#g19" ] []
            , use [ y "340", xlinkHref "#g37" ] []
            , use [ y "357", xlinkHref "#g60" ] []
            , use [ y "374", xlinkHref "#g19" ] []
            , use [ y "391", xlinkHref "#g37" ] []
            , use [ y "408", xlinkHref "#g68" ] []
            , use [ y "425", xlinkHref "#g69" ] []
            , use [ y "442", xlinkHref "#g70" ] []
            , use [ y "459", xlinkHref "#g71" ] []
            , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "29750" "428"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g20" ] []
            , use [ y "17", xlinkHref "#g47" ] []
            , use [ y "34", xlinkHref "#g19" ] []
            , use [ y "51", xlinkHref "#g37" ] []
            , use [ y "68", xlinkHref "#g48" ] []
            , use [ y "85", xlinkHref "#g49" ] []
            , use [ y "102", xlinkHref "#g50" ] []
            , use [ y "119", xlinkHref "#g51" ] []
            , use [ y "136", xlinkHref "#g52" ] []
            , use [ y "153", xlinkHref "#g53" ] []
            , use [ y "187", xlinkHref "#g54" ] []
            , use [ y "204", xlinkHref "#g19" ] []
            , use [ y "221", xlinkHref "#g37" ] []
            , use [ y "238", xlinkHref "#g55" ] []
            , use [ y "255", xlinkHref "#g56" ] []
            , use [ y "272", xlinkHref "#g57" ] []
            , use [ y "289", xlinkHref "#g58" ] []
            , use [ y "306", xlinkHref "#g19" ] []
            , use [ y "323", xlinkHref "#g37" ] []
            , use [ y "340", xlinkHref "#g60" ] []
            , use [ y "357", xlinkHref "#g19" ] []
            , use [ y "374", xlinkHref "#g37" ] []
            , use [ y "391", xlinkHref "#g68" ] []
            , use [ y "408", xlinkHref "#g69" ] []
            , use [ y "425", xlinkHref "#g70" ] []
            , use [ y "442", xlinkHref "#g71" ] []
            , use [ y "476", xlinkHref "#g72" ] []
            , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "30178" "7"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g47" ] []
            , use [ y "17", xlinkHref "#g19" ] []
            , use [ y "34", xlinkHref "#g37" ] []
            , use [ y "51", xlinkHref "#g48" ] []
            , use [ y "68", xlinkHref "#g49" ] []
            , use [ y "85", xlinkHref "#g50" ] []
            , use [ y "102", xlinkHref "#g51" ] []
            , use [ y "119", xlinkHref "#g52" ] []
            , use [ y "136", xlinkHref "#g53" ] []
            , use [ y "170", xlinkHref "#g54" ] []
            , use [ y "187", xlinkHref "#g19" ] []
            , use [ y "204", xlinkHref "#g37" ] []
            , use [ y "221", xlinkHref "#g55" ] []
            , use [ y "238", xlinkHref "#g56" ] []
            , use [ y "255", xlinkHref "#g57" ] []
            , use [ y "272", xlinkHref "#g58" ] []
            , use [ y "289", xlinkHref "#g19" ] []
            , use [ y "306", xlinkHref "#g37" ] []
            , use [ y "323", xlinkHref "#g60" ] []
            , use [ y "340", xlinkHref "#g19" ] []
            , use [ y "357", xlinkHref "#g37" ] []
            , use [ y "374", xlinkHref "#g68" ] []
            , use [ y "391", xlinkHref "#g69" ] []
            , use [ y "408", xlinkHref "#g70" ] []
            , use [ y "425", xlinkHref "#g71" ] []
            , use [ y "459", xlinkHref "#g72" ] []
            , use [ y "476", xlinkHref "#g73" ] []
            , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "30185" "159"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g19" ] []
            , use [ y "17", xlinkHref "#g37" ] []
            , use [ y "34", xlinkHref "#g48" ] []
            , use [ y "51", xlinkHref "#g49" ] []
            , use [ y "68", xlinkHref "#g50" ] []
            , use [ y "85", xlinkHref "#g51" ] []
            , use [ y "102", xlinkHref "#g52" ] []
            , use [ y "119", xlinkHref "#g53" ] []
            , use [ y "153", xlinkHref "#g54" ] []
            , use [ y "170", xlinkHref "#g19" ] []
            , use [ y "187", xlinkHref "#g37" ] []
            , use [ y "204", xlinkHref "#g55" ] []
            , use [ y "221", xlinkHref "#g56" ] []
            , use [ y "238", xlinkHref "#g57" ] []
            , use [ y "255", xlinkHref "#g58" ] []
            , use [ y "272", xlinkHref "#g19" ] []
            , use [ y "289", xlinkHref "#g37" ] []
            , use [ y "306", xlinkHref "#g60" ] []
            , use [ y "323", xlinkHref "#g19" ] []
            , use [ y "340", xlinkHref "#g37" ] []
            , use [ y "357", xlinkHref "#g68" ] []
            , use [ y "374", xlinkHref "#g69" ] []
            , use [ y "391", xlinkHref "#g70" ] []
            , use [ y "408", xlinkHref "#g71" ] []
            , use [ y "442", xlinkHref "#g72" ] []
            , use [ y "459", xlinkHref "#g73" ] []
            , use [ y "476", xlinkHref "#g74" ] []
            , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "30344" "26"
            ]
        , g [ display "none" ]
            [ rect
                [ class "foreground"
                , height "17"
                , width "8"
                , x "0"
                , y "493"
                ]
                []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "30370" "8"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g49" ] []
            , use [ y "17", xlinkHref "#g50" ] []
            , use [ y "34", xlinkHref "#g51" ] []
            , use [ y "51", xlinkHref "#g52" ] []
            , use [ y "68", xlinkHref "#g53" ] []
            , use [ y "102", xlinkHref "#g54" ] []
            , use [ y "119", xlinkHref "#g19" ] []
            , use [ y "136", xlinkHref "#g37" ] []
            , use [ y "153", xlinkHref "#g55" ] []
            , use [ y "170", xlinkHref "#g56" ] []
            , use [ y "187", xlinkHref "#g57" ] []
            , use [ y "204", xlinkHref "#g58" ] []
            , use [ y "221", xlinkHref "#g19" ] []
            , use [ y "238", xlinkHref "#g37" ] []
            , use [ y "255", xlinkHref "#g60" ] []
            , use [ y "272", xlinkHref "#g19" ] []
            , use [ y "289", xlinkHref "#g37" ] []
            , use [ y "306", xlinkHref "#g68" ] []
            , use [ y "323", xlinkHref "#g69" ] []
            , use [ y "340", xlinkHref "#g70" ] []
            , use [ y "357", xlinkHref "#g71" ] []
            , use [ y "391", xlinkHref "#g72" ] []
            , use [ y "408", xlinkHref "#g73" ] []
            , use [ y "425", xlinkHref "#g74" ] []
            , use [ y "442", xlinkHref "#g75" ] []
            , use [ y "476", xlinkHref "#g76" ] []
            , animateElem "30370" "14"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "30378" "6"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
            , use [ y "493", xlinkHref "#g2" ] []
            , animateElem "30384" "957"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "24", y "493" ] []
            , use [ y "493", xlinkHref "#g28" ] []
            , animateElem "31341" "216"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "32", y "493" ] []
            , use [ y "493", xlinkHref "#g29" ] []
            , animateElem "31557" "231"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "40", y "493" ] []
            , use [ y "493", xlinkHref "#g30" ] []
            , animateElem "31788" "101"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "48", y "493" ] []
            , use [ y "493", xlinkHref "#g31" ] []
            , animateElem "31889" "341"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "56", y "493" ] []
            , use [ y "493", xlinkHref "#g32" ] []
            , animateElem "32230" "293"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "64", y "493" ] []
            , use [ y "493", xlinkHref "#g77" ] []
            , animateElem "32523" "125"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "72", y "493" ] []
            , use [ y "493", xlinkHref "#g78" ] []
            , animateElem "32648" "178"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "80", y "493" ] []
            , use [ y "493", xlinkHref "#g79" ] []
            , animateElem "32826" "149"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "88", y "493" ] []
            , use [ y "493", xlinkHref "#g80" ] []
            , animateElem "32975" "443"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "96", y "493" ] []
            , use [ y "493", xlinkHref "#g81" ] []
            , animateElem "33418" "459"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "104", y "493" ] []
            , use [ y "493", xlinkHref "#g82" ] []
            , animateElem "33877" "616"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "112", y "493" ] []
            , use [ y "493", xlinkHref "#g83" ] []
            , animateElem "34493" "434"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "120", y "493" ] []
            , use [ y "493", xlinkHref "#g84" ] []
            , animateElem "34927" "252"
            ]
        , g [ display "none" ]
            [ rect [ class "foreground", height "17", width "8", x "128", y "493" ] []
            , use [ y "493", xlinkHref "#g85" ] []
            , animateElem "35179" "160"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g51" ] []
            , use [ y "17", xlinkHref "#g52" ] []
            , use [ y "34", xlinkHref "#g53" ] []
            , use [ y "68", xlinkHref "#g54" ] []
            , use [ y "85", xlinkHref "#g19" ] []
            , use [ y "102", xlinkHref "#g37" ] []
            , use [ y "119", xlinkHref "#g55" ] []
            , use [ y "136", xlinkHref "#g56" ] []
            , use [ y "153", xlinkHref "#g57" ] []
            , use [ y "170", xlinkHref "#g58" ] []
            , use [ y "187", xlinkHref "#g19" ] []
            , use [ y "204", xlinkHref "#g37" ] []
            , use [ y "221", xlinkHref "#g60" ] []
            , use [ y "238", xlinkHref "#g19" ] []
            , use [ y "255", xlinkHref "#g37" ] []
            , use [ y "272", xlinkHref "#g68" ] []
            , use [ y "289", xlinkHref "#g69" ] []
            , use [ y "306", xlinkHref "#g70" ] []
            , use [ y "323", xlinkHref "#g71" ] []
            , use [ y "357", xlinkHref "#g72" ] []
            , use [ y "374", xlinkHref "#g73" ] []
            , use [ y "391", xlinkHref "#g74" ] []
            , use [ y "408", xlinkHref "#g75" ] []
            , use [ y "442", xlinkHref "#g76" ] []
            , use [ y "459", xlinkHref "#g19" ] []
            , use [ y "476", xlinkHref "#g86" ] []
            , animateElem "30384" "4956"
            ]
        , g [ display "none" ]
            [ rect
                [ class "foreground"
                , height "17"
                , width "8"
                , x "128"
                , y "493"
                ]
                []
            , use [ y "493", xlinkHref "#g85" ] []
            , animateElem "35339" "1"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g52" ] []
            , use [ y "17", xlinkHref "#g53" ] []
            , use [ y "51", xlinkHref "#g54" ] []
            , use [ y "68", xlinkHref "#g19" ] []
            , use [ y "85", xlinkHref "#g37" ] []
            , use [ y "102", xlinkHref "#g55" ] []
            , use [ y "119", xlinkHref "#g56" ] []
            , use [ y "136", xlinkHref "#g57" ] []
            , use [ y "153", xlinkHref "#g58" ] []
            , use [ y "170", xlinkHref "#g19" ] []
            , use [ y "187", xlinkHref "#g37" ] []
            , use [ y "204", xlinkHref "#g60" ] []
            , use [ y "221", xlinkHref "#g19" ] []
            , use [ y "238", xlinkHref "#g37" ] []
            , use [ y "255", xlinkHref "#g68" ] []
            , use [ y "272", xlinkHref "#g69" ] []
            , use [ y "289", xlinkHref "#g70" ] []
            , use [ y "306", xlinkHref "#g71" ] []
            , use [ y "340", xlinkHref "#g72" ] []
            , use [ y "357", xlinkHref "#g73" ] []
            , use [ y "374", xlinkHref "#g74" ] []
            , use [ y "391", xlinkHref "#g75" ] []
            , use [ y "425", xlinkHref "#g76" ] []
            , use [ y "442", xlinkHref "#g19" ] []
            , use [ y "459", xlinkHref "#g86" ] []
            , use [ y "476", xlinkHref "#g87" ] []
            , rect
                [ class "foreground"
                , height "17"
                , width "8"
                , x "0"
                , y "493"
                ]
                []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "35340" "5"
            ]
        , g [ display "none" ]
            [ rect
                [ class "foreground"
                , height "17"
                , width "8"
                , x "0"
                , y "493"
                ]
                []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "35345" "8"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g56" ] []
            , use [ y "17", xlinkHref "#g57" ] []
            , use [ y "34", xlinkHref "#g58" ] []
            , use [ y "51", xlinkHref "#g19" ] []
            , use [ y "68", xlinkHref "#g37" ] []
            , use [ y "85", xlinkHref "#g60" ] []
            , use [ y "102", xlinkHref "#g19" ] []
            , use [ y "119", xlinkHref "#g37" ] []
            , use [ y "136", xlinkHref "#g68" ] []
            , use [ y "153", xlinkHref "#g69" ] []
            , use [ y "170", xlinkHref "#g70" ] []
            , use [ y "187", xlinkHref "#g71" ] []
            , use [ y "221", xlinkHref "#g72" ] []
            , use [ y "238", xlinkHref "#g73" ] []
            , use [ y "255", xlinkHref "#g74" ] []
            , use [ y "272", xlinkHref "#g75" ] []
            , use [ y "306", xlinkHref "#g76" ] []
            , use [ y "323", xlinkHref "#g19" ] []
            , use [ y "340", xlinkHref "#g86" ] []
            , use [ y "357", xlinkHref "#g87" ] []
            , use [ y "374", xlinkHref "#g88" ] []
            , use [ y "391", xlinkHref "#g89" ] []
            , use [ y "408", xlinkHref "#g90" ] []
            , use [ y "425", xlinkHref "#g91" ] []
            , use [ y "442", xlinkHref "#g92" ] []
            , use [ y "476", xlinkHref "#g93" ] []
            , animateElem "35345" "14"
            ]
        , g [ display "none" ]
            [ rect
                [ class "foreground"
                , height "17"
                , width "8"
                , x "0"
                , y "493"
                ]
                []
            , use [ y "493", xlinkHref "#g1" ] []
            , animateElem "35353" "6"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g58" ] []
            , use [ y "17", xlinkHref "#g19" ] []
            , use [ y "34", xlinkHref "#g37" ] []
            , use [ y "51", xlinkHref "#g60" ] []
            , use [ y "68", xlinkHref "#g19" ] []
            , use [ y "85", xlinkHref "#g37" ] []
            , use [ y "102", xlinkHref "#g68" ] []
            , use [ y "119", xlinkHref "#g69" ] []
            , use [ y "136", xlinkHref "#g70" ] []
            , use [ y "153", xlinkHref "#g71" ] []
            , use [ y "187", xlinkHref "#g72" ] []
            , use [ y "204", xlinkHref "#g73" ] []
            , use [ y "221", xlinkHref "#g74" ] []
            , use [ y "238", xlinkHref "#g75" ] []
            , use [ y "272", xlinkHref "#g76" ] []
            , use [ y "289", xlinkHref "#g19" ] []
            , use [ y "306", xlinkHref "#g86" ] []
            , use [ y "323", xlinkHref "#g87" ] []
            , use [ y "340", xlinkHref "#g88" ] []
            , use [ y "357", xlinkHref "#g89" ] []
            , use [ y "374", xlinkHref "#g90" ] []
            , use [ y "391", xlinkHref "#g91" ] []
            , use [ y "408", xlinkHref "#g92" ] []
            , use [ y "442", xlinkHref "#g93" ] []
            , use [ y "459", xlinkHref "#g19" ] []
            , use [ y "476", xlinkHref "#g37" ] []
            , rect [ class "foreground", height "17", width "8", x "16", y "493" ] []
            , use [ y "493", xlinkHref "#g2" ] []
            , animateElem "35359" "1587"
            ]
        , g [ display "none" ]
            [ use [ y "0", xlinkHref "#g19" ] []
            , use [ y "17", xlinkHref "#g37" ] []
            , use [ y "34", xlinkHref "#g60" ] []
            , use [ y "51", xlinkHref "#g19" ] []
            , use [ y "68", xlinkHref "#g37" ] []
            , use [ y "85", xlinkHref "#g68" ] []
            , use [ y "102", xlinkHref "#g69" ] []
            , use [ y "119", xlinkHref "#g70" ] []
            , use [ y "136", xlinkHref "#g71" ] []
            , use [ y "170", xlinkHref "#g72" ] []
            , use [ y "187", xlinkHref "#g73" ] []
            , use [ y "204", xlinkHref "#g74" ] []
            , use [ y "221", xlinkHref "#g75" ] []
            , use [ y "255", xlinkHref "#g76" ] []
            , use [ y "272", xlinkHref "#g19" ] []
            , use [ y "289", xlinkHref "#g86" ] []
            , use [ y "306", xlinkHref "#g87" ] []
            , use [ y "323", xlinkHref "#g88" ] []
            , use [ y "340", xlinkHref "#g89" ] []
            , use [ y "357", xlinkHref "#g90" ] []
            , use [ y "374", xlinkHref "#g91" ] []
            , use [ y "391", xlinkHref "#g92" ] []
            , use [ y "425", xlinkHref "#g93" ] []
            , use [ y "442", xlinkHref "#g19" ] []
            , use [ y "459", xlinkHref "#g37" ] []
            , use [ y "476", xlinkHref "#g94" ] []
            , rect [ class "foreground", height "17", width "8", x "0", y "493" ] []
            , use [ y "493", xlinkHref "#g1" ] []
            , animate
                [ attributeName "display"
                , begin "36946ms; anim_last.end+36946ms"
                , dur "1000ms"
                , from "inline"
                , to "inline"
                , id "anim_last"
                ]
                []
            ]
        ]
